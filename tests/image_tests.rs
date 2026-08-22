//! Image-processing contract tests.
//!
//! Every test drives the processor functions directly (no HTTP). All
//! fixtures are generated in memory during the test run. Lossy output is
//! never compared byte-for-byte: assertions cover container signatures,
//! dimensions, alpha behavior, and pixel values with documented tolerances.
//!
//! Documented tolerances:
//! - Flattened-to-white JPEG pixels: every RGB channel >= 240.
//! - Preserved transparency: fully transparent source pixels keep alpha
//!   <= 16 in the output; fully opaque source pixels keep alpha >= 240.

use std::io::Cursor;

use image::{DynamicImage, GenericImageView, RgbImage, RgbaImage};
use libvips::VipsImage;
use pixtega::errors::ProcessError;
use pixtega::processor::{init_vips, process_image, verify_encoders};
use pixtega::types::{OutputFormat, Transform};

// ---------------------------------------------------------------- helpers

fn tf(width: u32, format: OutputFormat, quality: u32) -> Transform {
    Transform {
        width,
        format,
        quality,
    }
}

/// Deterministic RGB gradient so lossy encoders have real content to work
/// with (a flat color would compress to almost nothing at any quality).
fn gradient_rgb(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            ((x * 255) / width.max(1)) as u8,
            ((y * 255) / height.max(1)) as u8,
            (((x + y) * 255) / (width + height).max(1)) as u8,
        ])
    })
}

/// Deterministic per-pixel noise; used where output size must respond to
/// the quality setting.
fn noise_rgb(width: u32, height: u32) -> RgbImage {
    let mut state: u32 = 0x9e37_79b9;
    RgbImage::from_fn(width, height, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        image::Rgb([
            (state & 0xff) as u8,
            ((state >> 8) & 0xff) as u8,
            ((state >> 16) & 0xff) as u8,
        ])
    })
}

fn encode_jpeg(img: &RgbImage) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("fixture JPEG encode");
    buf.into_inner()
}

fn jpeg_fixture(width: u32, height: u32) -> Vec<u8> {
    encode_jpeg(&gradient_rgb(width, height))
}

fn png_fixture(width: u32, height: u32) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(gradient_rgb(width, height))
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("fixture PNG encode");
    buf.into_inner()
}

/// 64x64 RGBA PNG: fully transparent red everywhere except a fully opaque
/// blue 24x24 center square. Corners are transparent; the center is opaque.
fn transparent_png_fixture() -> Vec<u8> {
    let img = RgbaImage::from_fn(64, 64, |x, y| {
        if (20..44).contains(&x) && (20..44).contains(&y) {
            image::Rgba([0, 0, 255, 255])
        } else {
            image::Rgba([255, 0, 0, 0])
        }
    });
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("fixture PNG encode");
    buf.into_inner()
}

/// Splice a minimal hand-built APP1/EXIF segment (little-endian TIFF, one
/// IFD entry: tag 0x0112 Orientation) directly after the JPEG SOI marker.
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "fixture must be a JPEG");
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II"); // little-endian byte order
    tiff.extend_from_slice(&42u16.to_le_bytes()); // TIFF magic
    tiff.extend_from_slice(&8u32.to_le_bytes()); // offset of IFD0
    tiff.extend_from_slice(&1u16.to_le_bytes()); // one IFD entry
    tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation tag
    tiff.extend_from_slice(&3u16.to_le_bytes()); // type SHORT
    tiff.extend_from_slice(&1u32.to_le_bytes()); // count
    tiff.extend_from_slice(&orientation.to_le_bytes()); // value
    tiff.extend_from_slice(&0u16.to_le_bytes()); // value padding
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

    let mut payload = Vec::from(&b"Exif\0\0"[..]);
    payload.extend_from_slice(&tiff);

    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// CRC-32 (IEEE) needed to patch a PNG IHDR chunk by hand.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A structurally valid PNG whose IHDR claims absurd dimensions. Only the
/// header is ever read: the megapixel check must reject the image before
/// any pixel decode is attempted (the IDAT data for these dimensions does
/// not exist).
fn huge_header_png(width: u32, height: u32) -> Vec<u8> {
    let mut png = png_fixture(1, 1);
    // Layout: 8 signature bytes, 4 length, 4 "IHDR", 13 data, 4 CRC.
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32(&png[12..29]); // over chunk type + data
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

/// Two-frame animated GIF built with the image crate.
fn animated_gif_fixture() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut buf);
        for shade in [0u8, 255u8] {
            let frame_img = RgbaImage::from_pixel(16, 16, image::Rgba([shade, shade, shade, 255]));
            encoder
                .encode_frame(image::Frame::new(frame_img))
                .expect("fixture GIF frame");
        }
    }
    buf.into_inner()
}

/// Animated WebP built through libvips itself (load the GIF with all pages,
/// then webpsave the multi-page image). GIF is rejected by the loader
/// whitelist, so this is the way to exercise the animation rule on an
/// otherwise accepted container. `new_from_buffer` with an `n=-1` option
/// string is used instead of `ops::gifload_buffer_with_opts`, which
/// segfaults in libvips 1.6.1 (it passes the read-only `flags` property
/// through the varargs setter).
fn animated_webp_fixture() -> Vec<u8> {
    let gif = animated_gif_fixture();
    let all_pages = VipsImage::new_from_buffer(&gif, "n=-1").expect("fixture: gifload all pages");
    let webp = libvips::ops::webpsave_buffer(&all_pages).expect("fixture: animated webpsave");
    assert!(
        webp.len() > 20 && &webp[12..16] == b"VP8X" && (webp[20] & 0x02) != 0,
        "fixture must actually be an animated WebP"
    );
    webp
}

fn decode_with_image_crate(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory(bytes).expect("output must decode with the image crate")
}

/// AVIF cannot be decoded by the image crate; inspect it with libvips.
fn avif_properties(bytes: &[u8]) -> (i32, i32, bool) {
    let img = VipsImage::new_from_buffer(bytes, "").expect("output AVIF must load in vips");
    (img.get_width(), img.get_height(), img.image_hasalpha())
}

fn assert_jpeg_signature(bytes: &[u8]) {
    assert!(
        bytes.len() > 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF,
        "missing JPEG SOI signature"
    );
}

fn assert_webp_signature(bytes: &[u8]) {
    assert!(
        bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "missing RIFF/WEBP container signature"
    );
}

fn assert_avif_signature(bytes: &[u8]) {
    assert!(
        bytes.len() > 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"avif",
        "missing ftyp/avif container signature"
    );
}

#[track_caller]
fn assert_undecodable(result: Result<Vec<u8>, ProcessError>, what: &str) {
    match result {
        Err(err @ ProcessError::Undecodable { .. }) => {
            assert_eq!(err.status(), 502, "{what}: undecodable must be 502-class");
        }
        Err(other) => panic!("{what}: expected Undecodable, got {other:?}"),
        Ok(_) => panic!("{what}: expected Undecodable, got success"),
    }
}

#[track_caller]
fn assert_too_many_pixels(result: Result<Vec<u8>, ProcessError>, what: &str) {
    match result {
        Err(err @ ProcessError::TooManyPixels) => {
            assert_eq!(err.status(), 502, "{what}: TooManyPixels must be 502-class");
        }
        Err(other) => panic!("{what}: expected TooManyPixels, got {other:?}"),
        Ok(_) => panic!("{what}: expected TooManyPixels, got success"),
    }
}

const MP: u64 = 100; // generous default megapixel limit for happy paths

// ------------------------------------------------------------------ tests

#[test]
fn downscale_preserves_aspect_ratio() {
    init_vips();
    let source = jpeg_fixture(2000, 1000);
    let out = process_image(&source, &tf(500, OutputFormat::Webp, 80), MP).unwrap();
    let decoded = decode_with_image_crate(&out);
    assert_eq!(decoded.dimensions(), (500, 250));
}

#[test]
fn narrow_source_is_never_upscaled() {
    init_vips();
    let source = jpeg_fixture(400, 300);
    for format in OutputFormat::all() {
        let out = process_image(&source, &tf(1920, format, 80), MP).unwrap();
        let (w, h) = match format {
            OutputFormat::Avif => {
                let (w, h, _) = avif_properties(&out);
                (w as u32, h as u32)
            }
            _ => decode_with_image_crate(&out).dimensions(),
        };
        assert_eq!(
            (w, h),
            (400, 300),
            "{format}: source narrower than the request keeps its dimensions"
        );
    }
}

#[test]
fn outputs_have_requested_dimensions_and_container_signatures() {
    init_vips();
    let source = jpeg_fixture(800, 600);
    for format in OutputFormat::all() {
        let out = process_image(&source, &tf(320, format, 80), MP).unwrap();
        match format {
            OutputFormat::Jpeg => {
                assert_jpeg_signature(&out);
                assert_eq!(decode_with_image_crate(&out).dimensions(), (320, 240));
            }
            OutputFormat::Webp => {
                assert_webp_signature(&out);
                assert_eq!(decode_with_image_crate(&out).dimensions(), (320, 240));
            }
            OutputFormat::Avif => {
                assert_avif_signature(&out);
                let (w, h, _) = avif_properties(&out);
                assert_eq!((w, h), (320, 240));
            }
        }
    }
}

#[test]
fn transparent_source_flattens_to_white_for_jpeg() {
    init_vips();
    let source = transparent_png_fixture();
    let out = process_image(&source, &tf(64, OutputFormat::Jpeg, 85), MP).unwrap();
    assert_jpeg_signature(&out);
    let decoded = decode_with_image_crate(&out).to_rgb8();
    assert_eq!(decoded.dimensions(), (64, 64));
    // The source corners are fully transparent RED; if flattening onto
    // white were skipped (or used the wrong background), green/blue would
    // be near 0. Tolerance: every channel >= 240.
    for (x, y) in [(1u32, 1u32), (62, 1), (1, 62), (62, 62)] {
        let p = decoded.get_pixel(x, y);
        assert!(
            p.0.iter().all(|&c| c >= 240),
            "corner ({x},{y}) = {:?} must be ~white (each channel >= 240)",
            p.0
        );
    }
}

#[test]
fn transparency_survives_webp() {
    init_vips();
    let source = transparent_png_fixture();
    let out = process_image(&source, &tf(64, OutputFormat::Webp, 80), MP).unwrap();
    assert_webp_signature(&out);
    let decoded = decode_with_image_crate(&out).to_rgba8();
    assert_eq!(decoded.dimensions(), (64, 64));
    assert!(
        decoded.get_pixel(1, 1).0[3] <= 16,
        "transparent corner must keep alpha <= 16, got {}",
        decoded.get_pixel(1, 1).0[3]
    );
    assert!(
        decoded.get_pixel(32, 32).0[3] >= 240,
        "opaque center must keep alpha >= 240, got {}",
        decoded.get_pixel(32, 32).0[3]
    );
}

#[test]
fn transparency_survives_avif() {
    init_vips();
    let source = transparent_png_fixture();
    let out = process_image(&source, &tf(64, OutputFormat::Avif, 55), MP).unwrap();
    assert_avif_signature(&out);
    // The image crate cannot decode AVIF; verify through libvips instead.
    let img = VipsImage::new_from_buffer(&out, "").expect("output AVIF must load");
    assert!(
        img.image_hasalpha(),
        "AVIF output must keep its alpha channel"
    );
    assert_eq!(img.get_bands(), 4, "AVIF output must keep 4 bands (RGBA)");
}

#[test]
fn quality_parameter_reaches_the_encoder() {
    init_vips();
    let source = encode_jpeg(&noise_rgb(256, 256));
    let low = process_image(&source, &tf(256, OutputFormat::Jpeg, 70), MP).unwrap();
    let high = process_image(&source, &tf(256, OutputFormat::Jpeg, 92), MP).unwrap();
    assert!(
        high.len() > low.len(),
        "noisy content at q92 ({} bytes) must be larger than at q70 ({} bytes)",
        high.len(),
        low.len()
    );
}

#[test]
fn source_above_megapixel_limit_is_rejected() {
    init_vips();
    // 2000x1000 = 2.0 MP against a 1 MP limit.
    let source = jpeg_fixture(2000, 1000);
    assert_too_many_pixels(
        process_image(&source, &tf(500, OutputFormat::Webp, 80), 1),
        "2000x1000 with max 1 MP",
    );
    // 5000x300 = 1.5 MP: rejected at 1 MP, accepted at 2 MP (boundary is
    // enforced on the pixel product, not on either dimension alone).
    let long = jpeg_fixture(5000, 300);
    assert_too_many_pixels(
        process_image(&long, &tf(320, OutputFormat::Jpeg, 85), 1),
        "5000x300 with max 1 MP",
    );
    process_image(&long, &tf(320, OutputFormat::Jpeg, 85), 2)
        .expect("5000x300 fits within a 2 MP limit");
}

#[test]
fn dimension_check_uses_checked_arithmetic_before_decode() {
    init_vips();
    // A structurally valid PNG header claiming 100000x100000 (10^10 pixels,
    // 10000 MP). There are no pixels to decode, so a rejection proves the
    // limit is enforced from header metadata before decoding. Two i32
    // dimensions cannot overflow a u64 product, so the overflow arm of the
    // pixel product is unreachable by construction; the limit side is
    // exercised below.
    let huge = huge_header_png(100_000, 100_000);
    assert_too_many_pixels(
        process_image(&huge, &tf(320, OutputFormat::Webp, 80), 500),
        "100000x100000 header with max 500 MP",
    );
    // Documented choice: if max_source_megapixels * 1_000_000 overflows
    // u64, the comparison cannot be evaluated and the source is rejected
    // rather than silently accepted.
    let tiny = png_fixture(4, 4);
    assert_too_many_pixels(
        process_image(&tiny, &tf(4, OutputFormat::Webp, 80), u64::MAX),
        "limit arithmetic overflow",
    );
    // Control: the same tiny image passes under a sane limit.
    process_image(&tiny, &tf(4, OutputFormat::Webp, 80), 1).expect("4x4 fits within 1 MP");
}

#[test]
fn exif_orientation_is_applied_before_resize_and_metadata_is_stripped() {
    init_vips();
    let plain = jpeg_fixture(100, 50);
    let oriented = with_exif_orientation(&plain, 6); // 90 degrees: 100x50 -> 50x100

    // Without the tag the source stays 100x50 (and is not upscaled).
    let control = process_image(&plain, &tf(1920, OutputFormat::Jpeg, 85), MP).unwrap();
    assert_eq!(decode_with_image_crate(&control).dimensions(), (100, 50));

    // Orientation 6 swaps the axes: oriented size is 50x100, kept as-is
    // because 50 < 1920 (no upscale of the oriented width).
    let unresized = process_image(&oriented, &tf(1920, OutputFormat::Jpeg, 85), MP).unwrap();
    assert_eq!(decode_with_image_crate(&unresized).dimensions(), (50, 100));

    // Resize happens after orientation: w25 against oriented 50x100 halves
    // both oriented axes. (Resizing before orienting would give 25x12->12x25.)
    let resized = process_image(&oriented, &tf(25, OutputFormat::Jpeg, 85), MP).unwrap();
    assert_eq!(decode_with_image_crate(&resized).dimensions(), (25, 50));

    // Source metadata must be stripped: no EXIF marker survives.
    for out in [&unresized, &resized] {
        assert!(
            !out.windows(4).any(|w| w == b"Exif"),
            "output must not contain an Exif marker"
        );
    }
}

#[test]
fn animated_input_is_rejected() {
    init_vips();
    // Animated WebP: an accepted container carrying animation.
    let animated_webp = animated_webp_fixture();
    assert_undecodable(
        process_image(&animated_webp, &tf(320, OutputFormat::Jpeg, 85), MP),
        "animated WebP",
    );
    // Animated GIF: rejected regardless (GIF is not a whitelisted loader).
    let animated_gif = animated_gif_fixture();
    assert_undecodable(
        process_image(&animated_gif, &tf(320, OutputFormat::Jpeg, 85), MP),
        "animated GIF",
    );
}

#[test]
fn document_and_unsupported_raster_input_is_rejected() {
    init_vips();
    let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\" \
                width=\"64\" height=\"64\"><rect width=\"64\" height=\"64\" fill=\"red\"/></svg>";
    assert_undecodable(
        process_image(svg, &tf(320, OutputFormat::Webp, 80), MP),
        "SVG document",
    );

    let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n\
                trailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_undecodable(
        process_image(pdf, &tf(320, OutputFormat::Webp, 80), MP),
        "PDF document",
    );

    // TIFF is a real raster format but not in the accepted set.
    let mut tiff = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(gradient_rgb(32, 32))
        .write_to(&mut tiff, image::ImageFormat::Tiff)
        .expect("fixture TIFF encode");
    assert_undecodable(
        process_image(&tiff.into_inner(), &tf(320, OutputFormat::Webp, 80), MP),
        "TIFF raster",
    );

    // A HEIF container that is not AVIF (ftyp major brand heic).
    let mut heic = Vec::new();
    heic.extend_from_slice(&24u32.to_be_bytes());
    heic.extend_from_slice(b"ftypheic");
    heic.extend_from_slice(&0u32.to_be_bytes());
    heic.extend_from_slice(b"heicmif1");
    assert_undecodable(
        process_image(&heic, &tf(320, OutputFormat::Webp, 80), MP),
        "HEIC container",
    );
}

#[test]
fn invalid_image_bytes_are_undecodable() {
    init_vips();
    // Bytes no loader recognizes at all.
    assert_undecodable(
        process_image(
            b"this is definitely not an image",
            &tf(320, OutputFormat::Webp, 80),
            MP,
        ),
        "garbage bytes",
    );
    // A JPEG whose header parses but whose pixel data is cut off.
    let full = jpeg_fixture(600, 400);
    let truncated = &full[..full.len() / 2];
    assert_undecodable(
        process_image(truncated, &tf(320, OutputFormat::Webp, 80), MP),
        "truncated JPEG",
    );
}

#[test]
fn webp_and_avif_sources_are_accepted() {
    init_vips();
    // WebP source (image crate writes lossless WebP).
    let mut webp = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(gradient_rgb(300, 200))
        .write_to(&mut webp, image::ImageFormat::WebP)
        .expect("fixture WebP encode");
    let out = process_image(&webp.into_inner(), &tf(150, OutputFormat::Jpeg, 85), MP).unwrap();
    assert_eq!(decode_with_image_crate(&out).dimensions(), (150, 100));

    // AVIF source: derive one with the processor itself, then feed it back.
    let avif = process_image(
        &jpeg_fixture(300, 200),
        &tf(300, OutputFormat::Avif, 55),
        MP,
    )
    .unwrap();
    assert_avif_signature(&avif);
    let out = process_image(&avif, &tf(150, OutputFormat::Webp, 80), MP).unwrap();
    assert_eq!(decode_with_image_crate(&out).dimensions(), (150, 100));
}

/// A MIAF-style AVIF whose `ftyp` major brand is `mif1` and that carries
/// `avif` only in the compatible-brands list must still be accepted.
#[test]
fn avif_with_mif1_major_brand_is_accepted() {
    init_vips();
    let mut avif = process_image(
        &jpeg_fixture(300, 200),
        &tf(300, OutputFormat::Avif, 55),
        MP,
    )
    .unwrap();

    // Sanity: patching assumes an ISO-BMFF ftyp box at the start whose
    // compatible-brands list (bytes 16..box_len) includes `avif`.
    assert_eq!(&avif[4..8], b"ftyp", "fixture must start with ftyp");
    let box_len = u32::from_be_bytes([avif[0], avif[1], avif[2], avif[3]]) as usize;
    let (compat_brands, _) = avif[16..box_len].as_chunks::<4>();
    let compat_has_avif = compat_brands.iter().any(|b| b == b"avif" || b == b"avis");
    assert!(
        compat_has_avif,
        "encoder fixture lacks avif in compatible brands; test needs a new patch strategy"
    );

    avif[8..12].copy_from_slice(b"mif1");
    let out = process_image(&avif, &tf(150, OutputFormat::Webp, 80), MP)
        .expect("mif1-major AVIF must be accepted");
    assert_eq!(decode_with_image_crate(&out).dimensions(), (150, 100));

    // A container with no AVIF brand anywhere (HEIC-style) stays rejected.
    let mut heic = process_image(
        &jpeg_fixture(300, 200),
        &tf(300, OutputFormat::Avif, 55),
        MP,
    )
    .unwrap();
    let box_len = u32::from_be_bytes([heic[0], heic[1], heic[2], heic[3]]) as usize;
    heic[8..12].copy_from_slice(b"heic");
    for offset in (16..box_len).step_by(4) {
        if &heic[offset..offset + 4] == b"avif" || &heic[offset..offset + 4] == b"avis" {
            heic[offset..offset + 4].copy_from_slice(b"heic");
        }
    }
    assert_undecodable(
        process_image(&heic, &tf(150, OutputFormat::Webp, 80), MP),
        "heic-branded container",
    );
}

#[test]
fn verify_encoders_succeeds_for_every_enabled_format() {
    init_vips();
    verify_encoders(&OutputFormat::all()).expect("all enabled encoders must verify");
    verify_encoders(&[OutputFormat::Avif]).expect("AVIF encoder must verify at runtime");
}
