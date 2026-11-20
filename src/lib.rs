//! On-demand image derivation service.
//!
//! The behavioral contract (URL grammar, caching, error taxonomy) is
//! documented in the README and enforced by the test suite. Module shape:
//!
//! ```text
//! HTTP request
//!   -> request parser and policy   (request)
//!   -> Source registry             (sources)
//!   -> selected Source adapter     (sources::{http, filesystem, s3})
//!   -> image processor             (processor)
//!   -> HTTP response               (app)
//! ```

// `unsafe` is allowed only in two isolated FFI islands, each
// `#[allow(unsafe_code)]`: `processor::vips_ffi` (libvips) and
// `allocator::glibc` (the allocator). Any new unsafe block anywhere else
// fails the build.
#![deny(unsafe_code)]

pub mod allocator;
pub mod app;
pub mod config;
pub mod errors;
pub mod etag;
pub mod logging;
pub mod processor;
pub mod request;
pub mod sources;
pub mod types;
