//! Persistent cache system for dependency caching
//!
//! Provides content-addressed caching keyed by lockfile hashes.
//! Caches are immutable once finalized, ensuring tamper-proof builds.
//!
//! # Security Model
//!
//! - Cache keys derived from lockfile SHA256 hash
//! - Complete caches mounted read-only (immutable)
//! - Changing cache contents requires different lockfile = different hash
//! - Incomplete caches (from crashes) remain writable for retry
//!
//! # Cache States
//!
//! | State | Mount | Description |
//! |-------|-------|-------------|
//! | Miss | rw | No volume exists, creating new |
//! | Building | rw | In progress or crashed, retryable |
//! | Complete | ro | Finalized, immutable |

pub mod lockfile;
pub mod volume;

pub use lockfile::{detect_lockfiles, Ecosystem, LockfileInfo};
pub use volume::{labels, plan_cache_mounts, CacheMount, CacheState, CacheVolume};
