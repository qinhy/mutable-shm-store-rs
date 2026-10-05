use std::os::fd::OwnedFd;

use crate::error::{MStoreError, Result};
use crate::models::{AccessMode, MappingInfo};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::LinuxMemfdRegion;
#[cfg(target_os = "macos")]
pub use macos::MacOsSharedRegion;

pub struct ClientMapping {
    pub info: MappingInfo,
    pub fd: Option<OwnedFd>,
}

#[derive(Debug)]
pub enum Region {
    #[cfg(target_os = "linux")]
    Linux(LinuxMemfdRegion),
    #[cfg(target_os = "macos")]
    MacOs(MacOsSharedRegion),
}

impl Region {
    pub fn create(size: u64, object_id: &str) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            return Ok(Self::Linux(LinuxMemfdRegion::create(size, object_id)?));
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self::MacOs(MacOsSharedRegion::create(size, object_id)?));
        }
        #[allow(unreachable_code)]
        Err(MStoreError::Unsupported(
            "mstore-rs currently supports Linux and macOS shared-memory backends".into(),
        ))
    }

    pub fn client_mapping(&self, mode: AccessMode) -> Result<ClientMapping> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Linux(region) => region.client_mapping(mode),
            #[cfg(target_os = "macos")]
            Self::MacOs(region) => region.client_mapping(mode),
        }
    }
}
