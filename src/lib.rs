//! On-demand image derivation service.
//!
//! See SPEC.md for the full contract. Module shape:
//!
//! ```text
//! HTTP request
//!   -> request parser and policy   (request)
//!   -> Source registry             (sources)
//!   -> selected Source adapter     (sources::{http, filesystem, s3})
//!   -> image processor             (processor)
//!   -> HTTP response               (app)
//! ```

pub mod app;
pub mod config;
pub mod errors;
pub mod logging;
pub mod processor;
pub mod request;
pub mod sources;
pub mod types;
