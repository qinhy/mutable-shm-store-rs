//! `mstore` is a local mutable shared-memory object store.
//!
//! The control plane carries only metadata and capability tokens. Large payloads
//! live in OS shared-memory mappings and are handed to clients without copying
//! the payload through the daemon.

pub mod backend;
pub mod client;
pub mod error;
pub mod models;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod transport;

#[cfg(feature = "ndarray")]
pub mod array;

pub use client::{connect, CacheInfo, Client, SharedObject};
pub use error::{MStoreError, Result};
pub use models::{AccessMode, MappingInfo, ObjectInfo, Permission, TokenInfo};
pub use server::{default_endpoint, MStoreServer};

#[cfg(feature = "ndarray")]
pub use array::ShmElement;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
