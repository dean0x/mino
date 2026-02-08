//! Composable layer system
//!
//! Layers are reusable install scripts + manifests that can be combined
//! dynamically to build composed container images. Each layer adds a
//! language toolchain or tool set to the base image.

pub mod compose;
pub mod manifest;
pub mod resolve;

pub use compose::{compose_image, ComposedImageResult};
pub use manifest::LayerManifest;
pub use resolve::{resolve_layers, LayerScript, LayerSource, ResolvedLayer};
