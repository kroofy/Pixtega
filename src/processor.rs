//! Image processing with libvips.
//!
//! Accepts source bytes plus a resolved [`Transform`] and returns encoded
//! bytes or a typed [`ProcessError`]. Knows nothing about HTTP, Sources, or
//! configuration allowlists.
//!
//! Pipeline:
//!
//! 1. Identify the loader libvips would pick for the bytes
//!    (`vips_foreign_find_load_buffer`) and whitelist the JPEG, PNG, WebP,
//!    and HEIF buffer loaders only. SVG, PDF, GIF, TIFF, and every other
//!    loader is rejected as undecodable. For HEIF input the `ftyp` major
//!    brand must be `avif` or `avis`: this cheaply excludes HEIC without a
//!    decode. AVIF files that carry `avif` only as a compatible brand are
//!    conservatively rejected.
//! 2. Open the header only (no pixel decode) and reject animated or
//!    multi-page input (`n-pages > 1`; for WebP additionally the VP8X
//!    animation flag in case metadata is insufficient).
//! 3. Read width, height, and EXIF orientation from the header. An
//!    orientation of 5..=8 swaps the oriented width and height. Enforce
//!    `oriented_width * oriented_height <= max_source_megapixels *
//!    1_000_000` entirely in checked u64 arithmetic; any overflow or excess
//!    is [`ProcessError::TooManyPixels`]. This happens before any pixel is
//!    decoded.
//! 4. Decode, auto-rotate, and downscale in one fused `thumbnail_buffer`
//!    call (`size=Down` never upscales: a source narrower than the request
//!    keeps its dimensions; `no_rotate=false` applies EXIF orientation
//!    before the resize; `fail_on=Error` makes corrupt or truncated pixel
//!    data fail instead of decoding partially). The result is materialized
//!    with `vips_image_copy_memory`, which forces full evaluation, so
//!    decode errors surface here and not during encoding.
//! 5. For JPEG output only, flatten transparency onto white
//!    ([`ProcessError::Flatten`] on failure). WebP and AVIF preserve alpha.
//! 6. Encode with the resolved quality, stripping all source metadata
//!    (`strip=true`). AVIF is heifsave with AV1 compression at 8-bit depth.
//!
//! Error classification: everything up to and including the materialized
//! decode is attributed to the source bytes ([`ProcessError::Undecodable`],
//! 502) because no valid source image has been accepted yet and the
//! service's own arithmetic cannot fail there; failures after that point
//! are pipeline errors ([`ProcessError::Flatten`] / [`ProcessError::Encode`],
//! 500). [`ProcessError::Resize`] is reserved for explicit post-acceptance
//! geometry operations; the fused thumbnail pipeline performs none, so this
//! implementation never emits it.
//!
//! The process-wide libvips runtime is initialized exactly once via
//! [`init_vips`]. Native handles are never cloned or double-released: every
//! `VipsImage` is owned by exactly one binding wrapper and consumed or
//! dropped exactly once. libvips operations are thread-safe;
//! [`process_image`] is synchronous and safe to run from multiple blocking
//! threads. The libvips error buffer is process-global, so the `detail`
//! strings attached to errors are best-effort diagnostics only.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::sync::Once;

use libvips::ops::{
    self, FlattenOptions, ForeignHeifCompression, HeifsaveBufferOptions, JpegsaveBufferOptions,
    Size, ThumbnailBufferOptions, WebpsaveBufferOptions,
};
use libvips::{bindings, VipsApp, VipsImage};

use crate::errors::{ConfigError, ProcessError};
use crate::types::{OutputFormat, Transform};

/// Loader classes accepted for source bytes. `vips_foreign_find_load_buffer`
/// returns the GObject class name (e.g. "VipsForeignLoadJpegBuffer"); the
/// codec token between the "VipsForeignLoad" prefix and the "Buffer" suffix
/// is matched case-insensitively. "spng" is the libspng-based PNG loader
/// that libvips builds may prefer over libpng. "heif" is a candidate only:
/// the `ftyp` brand check below decides whether the container is actually
/// AVIF rather than HEIC.
const LOADER_TOKENS: [(&str, SniffedKind); 5] = [
    ("jpeg", SniffedKind::Jpeg),
    ("png", SniffedKind::Png),
    ("spng", SniffedKind::Png),
    ("webp", SniffedKind::Webp),
    ("heif", SniffedKind::Heif),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SniffedKind {
    Jpeg,
    Png,
    Webp,
    Heif,
}

static VIPS_INIT: Once = Once::new();

/// Initialize the process-wide libvips runtime. Safe to call more than
/// once; only the first call does work. Must be called before
/// [`process_image`].
pub fn init_vips() {
    VIPS_INIT.call_once(|| {
        let app = VipsApp::new("pixtega", false).expect("libvips runtime failed to initialize");
        // Modest per-operation thread pool: request-level parallelism comes
        // from max_concurrent_derivations, not from libvips worker threads.
        app.concurrency_set(2);
        // VipsApp's Drop calls vips_shutdown(), which must never run while
        // the process may still use libvips. Leak the handle on purpose.
        std::mem::forget(app);
    });
}

/// Verify at runtime that libvips can actually encode every enabled format
/// (a tiny in-memory encode per format). AVIF support in particular must
/// not be inferred from a successful compile. Called at startup;
/// failure stops the service.
pub fn verify_encoders(formats: &[OutputFormat]) -> Result<(), ConfigError> {
    init_vips();
    let probe = ops::black_with_opts(2, 2, &ops::BlackOptions { bands: 3 }).map_err(|e| {
        ConfigError::new(format!(
            "libvips cannot create a probe image: {}",
            describe_vips_error(&e)
        ))
    })?;
    for format in formats {
        encode(&probe, *format, 60).map_err(|_| {
            ConfigError::new(format!(
                "libvips cannot encode {}: encoder verification failed ({})",
                format,
                take_vips_error_buffer()
            ))
        })?;
    }
    Ok(())
}

/// Derive one image: decode `source_bytes`, apply the pipeline above, and
/// encode under `transform`.
///
/// `max_source_megapixels` bounds the oriented `width * height` (checked
/// u64 arithmetic, in units of 1_000_000 pixels). If either side of the
/// comparison overflows u64, the source is rejected as too large.
///
/// This is CPU-bound synchronous work; the HTTP layer runs it on a blocking
/// thread while holding a derivation permit.
pub fn process_image(
    source_bytes: &[u8],
    transform: &Transform,
    max_source_megapixels: u64,
) -> Result<Vec<u8>, ProcessError> {
    let kind = sniff_loader(source_bytes)?;

    if kind == SniffedKind::Heif && !is_avif_brand(source_bytes) {
        return Err(undecodable("HEIF container is not AVIF (ftyp brand)"));
    }

    // Header-only open: libvips reads metadata immediately but defers all
    // pixel decoding until pixels are pulled, which never happens on this
    // handle.
    let header = VipsImage::new_from_buffer(source_bytes, "").map_err(|e| {
        undecodable(format!(
            "cannot read image header: {}",
            describe_vips_error(&e)
        ))
    })?;

    if header.get_n_pages() > 1 {
        return Err(undecodable("animated or multi-page input"));
    }
    if kind == SniffedKind::Webp && webp_vp8x_animated(source_bytes) {
        return Err(undecodable("animated WebP input"));
    }

    let width = header.get_width();
    let height = header.get_height();
    if width <= 0 || height <= 0 {
        return Err(undecodable("image header reports empty dimensions"));
    }

    // EXIF orientation before limits: 5..=8 rotate by 90/270 degrees and
    // swap the oriented axes. The product is unchanged by the swap, but the
    // contract requires the check to apply to oriented dimensions.
    let (oriented_w, oriented_h) = if header.get_orientation() >= 5 {
        (height as u64, width as u64)
    } else {
        (width as u64, height as u64)
    };
    drop(header);

    let pixels = oriented_w.checked_mul(oriented_h);
    let limit = max_source_megapixels.checked_mul(1_000_000);
    match (pixels, limit) {
        (Some(p), Some(l)) if p <= l => {}
        _ => return Err(ProcessError::TooManyPixels),
    }

    // Fused decode + auto-rotate + downscale-only resize. `height` is set
    // to the operation maximum so the requested width is the only
    // constraint; `Size::Down` keeps a narrower source at its original
    // dimensions (never upscale).
    let target_width = transform.width.min(10_000_000) as i32;
    // Binding quirks, verified against libvips 8.15 + libvips-rust 1.6.1:
    // - The bindings pass every option unconditionally, and an empty
    //   profile string makes libvips try to load an ICC profile from the
    //   file "". Point both fallbacks at the built-in sRGB profile; output
    //   is then normalized to sRGB, which is what the encoders expect.
    // - The `fail_on` struct field does not reach the underlying loader,
    //   so corrupt input would decode partially. Passing `fail-on=error`
    //   through the loader option string does work; corrupt or truncated
    //   pixel data then errors at evaluation time below.
    // `no_rotate` is left at its default `false`: EXIF orientation is
    // applied before the resize.
    let thumb_options = ThumbnailBufferOptions {
        option_string: "fail-on=error".to_string(),
        height: 10_000_000,
        size: Size::Down,
        import_profile: "srgb".to_string(),
        export_profile: "srgb".to_string(),
        ..ThumbnailBufferOptions::default()
    };
    let resized = ops::thumbnail_buffer_with_opts(source_bytes, target_width, &thumb_options)
        .map_err(|e| undecodable(format!("cannot decode source: {}", describe_vips_error(&e))))?;

    // Force full evaluation now. Truncated or corrupt pixel data fails here
    // (a source problem), instead of surfacing later inside the encoder.
    let decoded = VipsImage::image_copy_memory(resized).map_err(|e| {
        undecodable(format!(
            "cannot decode source pixels: {}",
            describe_vips_error(&e)
        ))
    })?;

    // Alpha policy: WebP and AVIF preserve transparency; JPEG cannot carry
    // it, so flatten onto white — but only when alpha is actually present.
    let output = if transform.format == OutputFormat::Jpeg && decoded.image_hasalpha() {
        let flatten_options = FlattenOptions {
            background: vec![255.0, 255.0, 255.0],
            ..FlattenOptions::default()
        };
        ops::flatten_with_opts(&decoded, &flatten_options).map_err(|e| ProcessError::Flatten {
            detail: describe_vips_error(&e),
        })?
    } else {
        decoded
    };

    encode(&output, transform.format, transform.quality as i32).map_err(|e| ProcessError::Encode {
        detail: describe_vips_error(&e),
    })
}

/// Encode `image` as `format` at `quality`, stripping all source metadata.
fn encode(
    image: &VipsImage,
    format: OutputFormat,
    quality: i32,
) -> Result<Vec<u8>, libvips::error::Error> {
    match format {
        OutputFormat::Jpeg => ops::jpegsave_buffer_with_opts(
            image,
            &JpegsaveBufferOptions {
                q: quality,
                strip: true,
                ..JpegsaveBufferOptions::default()
            },
        ),
        OutputFormat::Webp => ops::webpsave_buffer_with_opts(
            image,
            &WebpsaveBufferOptions {
                q: quality,
                strip: true,
                ..WebpsaveBufferOptions::default()
            },
        ),
        OutputFormat::Avif => ops::heifsave_buffer_with_opts(
            image,
            &HeifsaveBufferOptions {
                q: quality,
                bitdepth: 8,
                compression: ForeignHeifCompression::Av1,
                strip: true,
                ..HeifsaveBufferOptions::default()
            },
        ),
    }
}

/// Ask libvips which buffer loader it would use for these bytes and map it
/// through the whitelist. `None` from libvips (no loader at all) and any
/// non-whitelisted loader are both undecodable.
fn sniff_loader(bytes: &[u8]) -> Result<SniffedKind, ProcessError> {
    let name = unsafe {
        let ptr = bindings::vips_foreign_find_load_buffer(
            bytes.as_ptr() as *const c_void,
            bytes.len() as _,
        );
        if ptr.is_null() {
            // find_load_buffer leaves a message in the global error buffer.
            let detail = take_vips_error_buffer();
            return Err(undecodable(format!(
                "no image loader accepts these bytes: {detail}"
            )));
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    let lower = name.to_ascii_lowercase();
    let codec = lower
        .strip_prefix("vipsforeignload")
        .and_then(|rest| rest.strip_suffix("buffer"))
        .unwrap_or("");
    LOADER_TOKENS
        .iter()
        .find(|(token, _)| codec == *token)
        .map(|(_, kind)| *kind)
        .ok_or_else(|| undecodable(format!("unsupported image loader: {name}")))
}

/// True when the bytes start with an ISO-BMFF `ftyp` box that declares an
/// AVIF brand — `avif` (still) or `avis` (sequence; sequences are then
/// rejected by the multi-page check) — as either the major brand or one of
/// the compatible brands. MIAF files (major brand `mif1`) commonly carry
/// `avif` only in the compatible-brands list; HEIC files declare neither
/// brand anywhere and stay rejected.
fn is_avif_brand(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let box_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = box_len.min(bytes.len());
    // Major brand at 8..12, minor version at 12..16, compatible brands after.
    let is_avif = |brand: &[u8; 4]| matches!(brand, b"avif" | b"avis");
    let major: &[u8; 4] = bytes[8..12].try_into().expect("length checked above");
    if is_avif(major) {
        return true;
    }
    let (compatible, _) = bytes[16..end].as_chunks::<4>();
    compatible.iter().any(is_avif)
}

/// True when a WebP RIFF container has an extended (VP8X) header with the
/// animation flag set. Used in addition to `n-pages` in case the loader
/// does not surface frame-count metadata from a header-only open.
fn webp_vp8x_animated(bytes: &[u8]) -> bool {
    bytes.len() > 20 && &bytes[12..16] == b"VP8X" && (bytes[20] & 0x02) != 0
}

fn undecodable(detail: impl Into<String>) -> ProcessError {
    ProcessError::Undecodable {
        detail: detail.into(),
    }
}

/// Best-effort diagnostic: binding error plus whatever is currently in the
/// process-global libvips error buffer (which is then cleared). Concurrent
/// operations may interleave buffer contents; details are for logs only.
fn describe_vips_error(err: &libvips::error::Error) -> String {
    let buffer = take_vips_error_buffer();
    if buffer.is_empty() {
        err.to_string()
    } else {
        format!("{err}: {buffer}")
    }
}

fn take_vips_error_buffer() -> String {
    unsafe {
        let ptr = bindings::vips_error_buffer();
        let message = if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().trim().to_string()
        };
        bindings::vips_error_clear();
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    // ------------------------------------------------ container sniffing

    /// Build an ISO-BMFF `ftyp` box: 4-byte length, "ftyp", major brand,
    /// minor version, compatible brands.
    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let len = 16 + 4 * compatible.len();
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(b"ftyp");
        out.extend_from_slice(major);
        out.extend_from_slice(&0u32.to_be_bytes());
        for brand in compatible {
            out.extend_from_slice(*brand);
        }
        out
    }

    #[test]
    fn avif_brand_accepts_major_and_compatible_brands() {
        assert!(is_avif_brand(&ftyp(b"avif", &[])));
        assert!(is_avif_brand(&ftyp(b"avis", &[])));
        assert!(is_avif_brand(&ftyp(b"mif1", &[b"miaf", b"avif"])));
        assert!(!is_avif_brand(&ftyp(b"heic", &[b"mif1"])));
        assert!(!is_avif_brand(&ftyp(b"mif1", &[b"heic"])));
    }

    /// A `ftyp` box with no compatible brands is exactly 16 bytes: the
    /// shortest input that can declare an AVIF major brand.
    #[test]
    fn avif_brand_accepts_the_minimum_sixteen_byte_box() {
        let minimal = ftyp(b"avif", &[]);
        assert_eq!(minimal.len(), 16);
        assert!(is_avif_brand(&minimal));
    }

    /// Boxes truncated below 16 bytes are rejected even when the bytes
    /// present spell `ftyp` and an AVIF brand: the minor version field is
    /// incomplete, so the container is malformed.
    #[test]
    fn avif_brand_rejects_truncated_boxes_and_missing_ftyp() {
        let truncated: Vec<u8> = ftyp(b"avif", &[])[..12].to_vec();
        assert_eq!(&truncated[4..8], b"ftyp");
        assert_eq!(&truncated[8..12], b"avif");
        assert!(!is_avif_brand(&truncated));
        assert!(!is_avif_brand(b""));
        assert!(!is_avif_brand(b"RIFF1234WEBPVP8 aaaa"));
    }

    /// A minimal RIFF/VP8X prefix: `webp_vp8x_animated` reads only the
    /// chunk fourcc at 12..16 and the flag byte at 20.
    fn vp8x_prefix(fourcc: &[u8; 4], flags: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&10u32.to_le_bytes());
        out.push(flags);
        out
    }

    #[test]
    fn vp8x_animation_flag_detection() {
        assert_eq!(vp8x_prefix(b"VP8X", 0x02).len(), 21);
        assert!(webp_vp8x_animated(&vp8x_prefix(b"VP8X", 0x02)));
        // Other VP8X feature bits alongside animation still count.
        assert!(webp_vp8x_animated(&vp8x_prefix(b"VP8X", 0x3E)));
        // Animation bit clear: extended header without animation.
        assert!(!webp_vp8x_animated(&vp8x_prefix(b"VP8X", 0x00)));
        assert!(!webp_vp8x_animated(&vp8x_prefix(b"VP8X", 0x3C)));
        // Simple lossy WebP: no VP8X chunk at all.
        assert!(!webp_vp8x_animated(&vp8x_prefix(b"VP8 ", 0x02)));
        // Exactly 20 bytes: the flag byte does not exist yet.
        assert!(!webp_vp8x_animated(&vp8x_prefix(b"VP8X", 0x02)[..20]));
        assert!(!webp_vp8x_animated(b""));
    }

    // ------------------------------------------------ vips error plumbing

    /// The libvips error buffer is process-global; tests that touch it must
    /// not interleave with each other.
    static VIPS_ERROR_BUFFER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Push `message` into the process-global libvips error buffer.
    fn set_vips_error(message: &str) {
        let domain = CString::new("pixtega-test").unwrap();
        let fmt = CString::new("%s").unwrap();
        let text = CString::new(message).unwrap();
        unsafe {
            bindings::vips_error(domain.as_ptr(), fmt.as_ptr(), text.as_ptr());
        }
    }

    #[test]
    fn take_vips_error_buffer_returns_and_clears_the_message() {
        init_vips();
        let _guard = VIPS_ERROR_BUFFER_LOCK.lock().unwrap();
        let _ = take_vips_error_buffer(); // drain anything left behind
        set_vips_error("buffered diagnostic");
        let taken = take_vips_error_buffer();
        assert!(
            taken.contains("buffered diagnostic"),
            "buffer content must be returned, got {taken:?}"
        );
        assert_eq!(
            take_vips_error_buffer(),
            "",
            "the buffer must be cleared by the first take"
        );
    }

    #[test]
    fn describe_vips_error_includes_binding_error_and_buffer() {
        init_vips();
        let _guard = VIPS_ERROR_BUFFER_LOCK.lock().unwrap();
        let err = VipsImage::new_from_buffer(b"not an image", "")
            .expect_err("garbage bytes must not load");
        let _ = take_vips_error_buffer(); // drain: exercise the empty case
        assert_eq!(describe_vips_error(&err), err.to_string());

        set_vips_error("extra context");
        let described = describe_vips_error(&err);
        assert!(
            described.starts_with(&err.to_string()),
            "description must start with the binding error, got {described:?}"
        );
        assert!(
            described.contains("extra context"),
            "description must include the buffer content, got {described:?}"
        );
    }
}
