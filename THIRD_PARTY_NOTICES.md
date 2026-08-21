# Third-party notices

This project is licensed under MIT OR Apache-2.0 (see `LICENSE-MIT` and
`LICENSE-APACHE`). It depends on third-party software with its own licenses.

## Native libraries (dynamically linked at runtime)

| Component | License | Role |
| --- | --- | --- |
| [libvips](https://github.com/libvips/libvips) | LGPL-2.1-or-later | image decode, resize, encode |
| [libheif](https://github.com/strukturag/libheif) | LGPL-3.0-or-later (plugins vary) | AVIF container support |
| [aom](https://aomedia.googlesource.com/aom/) | BSD-2-Clause + AOM patent license | AV1 (AVIF) encoding/decoding |
| [libwebp](https://chromium.googlesource.com/webm/libwebp) | BSD-3-Clause | WebP encoding/decoding |
| libjpeg-turbo / libpng / other libvips delegates | various permissive | JPEG/PNG support |

These libraries are not distributed in this repository. The container image
installs them from Ubuntu 24.04 packages; their license texts ship with
those packages under `/usr/share/doc/<package>/copyright`.

libvips is LGPL: this project links it dynamically and does not modify it,
which satisfies the LGPL for both the source build and the container image.

## Rust crates

Crate dependencies are declared in `Cargo.toml` and pinned in `Cargo.lock`.
All are available under permissive licenses (MIT/Apache-2.0/BSD or similar).
Generate a full inventory with:

```bash
cargo install cargo-license && cargo license
```
