//! Startup encoder verification contract.
//!
//! This test lives in its own integration binary (its own process) on
//! purpose, and deliberately never calls `init_vips` itself:
//! `verify_encoders` is the startup gate and its contract includes
//! initializing the libvips runtime and actually probing every enabled
//! encoder. If it silently did nothing, the first real derivation in this
//! process would fail — exactly what the second half of the test asserts.

use std::io::Cursor;

use image::{DynamicImage, RgbImage};
use pixtega::processor::{process_image, verify_encoders};
use pixtega::types::{OutputFormat, Transform};

#[test]
fn verify_encoders_initializes_the_runtime_and_probes_every_encoder() {
    verify_encoders(&OutputFormat::all()).expect("all enabled encoders must verify");

    // The service processes images right after startup verification with no
    // further initialization; that must work in this fresh process too.
    let img = RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30]));
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("fixture JPEG encode");
    let source = buf.into_inner();
    for format in OutputFormat::all() {
        process_image(
            &source,
            &Transform {
                width: 4,
                format,
                quality: 60,
            },
            100,
        )
        .unwrap_or_else(|err| panic!("{format}: derivation after startup verification: {err:?}"));
    }
}
