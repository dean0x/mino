//! Persistent cache system for dependency caching
//!
//! Provides content-addressed caching keyed by lockfile hashes.
//!
//! # Security Model
//!
//! - Cache keys derived from lockfile SHA256 hash
//! - Content-addressed: same lockfile = same cache volume
//! - Changing cache contents requires different lockfile = different hash
//! - Incomplete caches (from crashes) remain retryable
//!
//! # Cache States
//!
//! | State | Mount | Description |
//! |-------|-------|-------------|
//! | Miss | rw | No volume exists, creating new |
//! | Building | rw | In progress or crashed, retryable |
//! | Complete | rw | Finalized, skip re-finalization |

pub mod lockfile;
pub mod sidecar;
pub mod volume;

pub use lockfile::{detect_lockfiles, Ecosystem, LockfileInfo};
pub use sidecar::CacheSidecar;
pub use volume::{
    format_bytes, gb_to_bytes, labels, plan_cache_mounts, resolve_state, CacheMount,
    CacheSizeStatus, CacheState, CacheVolume,
};
