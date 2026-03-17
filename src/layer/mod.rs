//! Composable layer system
//!
//! Layers are reusable install scripts + manifests that can be combined
//! dynamically to build composed container images. Each layer adds a
//! language toolchain or tool set to the base image.

pub mod compose;
pub mod manifest;
pub mod resolve;

pub use compose::{compose_image, ComposedImageResult};
pub(crate) use compose::{compute_path_prepend, merge_layer_env, needs_compose_build};
pub(crate) use manifest::build_layer_manifest;
pub use manifest::LayerManifest;
pub use resolve::{
    list_available_layers, resolve_layers, AvailableLayer, LayerScript, LayerSource, ResolvedLayer,
};
