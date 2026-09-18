//! Feedback Board sidecar library.
//!
//! The crate owns the public feedback store and HTTP surface. It deliberately
//! has no dependency on `apps/core`: Core registers the app manifest and
//! proxies the sidecar through the generic ext-proxy/public-mount machinery.

pub mod admin;
pub mod api;
pub mod errors;
pub mod model;
pub mod paths;
pub mod public;
pub mod public_html;
pub mod store;
pub mod validation;

pub use api::{routes, Ctx};
pub use store::Store;
