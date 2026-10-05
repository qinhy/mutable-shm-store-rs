use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use uuid::Uuid;

use crate::backend::ClientMapping;
use crate::error::{MStoreError, Result};
use crate::models::{AccessMode, MappingInfo};

#[derive(Debug)]
pub struct MacOsSharedRegion {
    write_fd: OwnedFd,
    read_fd: OwnedFd,
    size: u64,
}

impl MacOsSharedRegion {
    pub fn create(size: u64, _object_id: &str) -> Result<Self> {
        if size == 0 || size > libc::off_t::MAX as u64 {
            return Err(MStoreError::InvalidRequest("invalid shared-memory size".into()));
        }
        let name = CString::new(format!("/mstore-{}", Uuid::new_v4().simple()))
            .expect("generated shm name contains no NUL");

        // SAFETY: name is NUL terminated and flags/mode are valid for shm_open.
        let write_raw = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if write_raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: shm_open returned a new owned descriptor.
        let write_fd = unsafe { OwnedFd::from_raw_fd(write_raw) };

        let setup = (|| -> Result<OwnedFd> {
            let rc = unsafe { libc::ftruncate(write_fd.as_raw_fd(), size as libc::off_t) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let read_raw = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
            if read_raw < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: shm_open returned a new owned descriptor.
            Ok(unsafe { OwnedFd::from_raw_fd(read_raw) })
        })();

        // Unlink immediately. Existing descriptors/mappings keep the object alive.
        unsafe { libc::shm_unlink(name.as_ptr()) };
        let read_fd = setup?;

        Ok(Self {
            write_fd,
            read_fd,
            size,
        })
    }

    fn dup(fd: &OwnedFd) -> Result<OwnedFd> {
        let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    pub fn client_mapping(&self, mode: AccessMode) -> Result<ClientMapping> {
        let fd = match mode {
            AccessMode::Read => Self::dup(&self.read_fd)?,
            AccessMode::Write => Self::dup(&self.write_fd)?,
        };
        Ok(ClientMapping {
            info: MappingInfo {
                backend: "posix_shm".into(),
                size: self.size,
                name: None,
                mode: None,
            },
            fd: Some(fd),
        })
    }
}
