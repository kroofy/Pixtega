//! Image processing with libvips.
//!
//! Accepts source bytes plus a resolved [`Transform`] and returns encoded
//! bytes or a typed [`ProcessError`]. Knows nothing about HTTP, Sources, or
//! configuration allowlists.
//!
//! Pipeline (SPEC.md "Image processing"):
//! metadata sniff (JPEG/PNG/WebP/AVIF raster only, no animation or
//! multi-page) -> EXIF orientation -> checked megapixel limit -> decode ->
//! downscale-only resize -> alpha handling (preserve for WebP/AVIF, flatten
//! onto white for JPEG) -> encode with the resolved quality, stripping
//! source metadata.
//!
//! The process-wide libvips runtime is initialized exactly once via
//! [`init_vips`]. Native handles are never cloned or double-released.

use crate::errors::{ConfigError, ProcessError};
use crate::types::{OutputFormat, Transform};

/// Initialize the process-wide libvips runtime. Safe to call more than
/// once; only the first call does work. Must be called before
/// [`process_image`].
pub fn init_vips() {
    todo!("implemented by the image-processing module")
}

/// Verify at runtime that libvips can actually encode every enabled format
/// (a tiny in-memory encode per format). AVIF support in particular must
/// not be inferred from a successful compile. Called at startup;
/// failure stops the service.
pub fn verify_encoders(formats: &[OutputFormat]) -> Result<(), ConfigError> {
    let _ = formats;
    todo!("implemented by the image-processing module")
}

/// Derive one image: decode `source_bytes`, apply the pipeline above, and
/// encode under `transform`.
///
/// `max_source_megapixels` bounds the oriented `width * height` (checked
/// u64 arithmetic, in units of 1_000_000 pixels).
///
/// This is CPU-bound synchronous work; the HTTP layer runs it on a blocking
/// thread while holding a derivation permit.
pub fn process_image(
    source_bytes: &[u8],
    transform: &Transform,
    max_source_megapixels: u64,
) -> Result<Vec<u8>, ProcessError> {
    let _ = (source_bytes, transform, max_source_megapixels);
    todo!("implemented by the image-processing module")
}
