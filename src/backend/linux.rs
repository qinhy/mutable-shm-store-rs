use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use crate::backend::ClientMapping;
use crate::error::{MStoreError, Result};
use crate::models::{AccessMode, MappingInfo};

#[derive(Debug)]
pub struct LinuxMemfdRegion {
    fd: OwnedFd,
    size: u64,
}

impl LinuxMemfdRegion {
    pub fn create(size: u64, object_id: &str) -> Result<Self> {
        if size == 0 || size > libc::off_t::MAX as u64 {
            return Err(MStoreError::InvalidRequest(
                "invalid shared-memory size".into(),
            ));
        }
        let name = CString::new(format!("mstore-{object_id}"))
            .map_err(|_| MStoreError::InvalidRequest("object id contains NUL".into()))?;

        // SAFETY: name is a valid NUL-terminated C string and flags are valid for memfd_create.
        let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC as libc::c_uint) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: successful memfd_create returns a newly owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        // SAFETY: fd is valid and size has already been bounded to off_t::MAX.
        let rc = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        Ok(Self { fd, size })
    }

    fn duplicate_write_fd(&self) -> Result<OwnedFd> {
        // F_DUPFD_CLOEXEC creates an independently owned descriptor while preserving O_RDWR.
        let raw = unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fcntl returned a fresh descriptor owned by this function.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn duplicate_read_fd(&self) -> Result<OwnedFd> {
        // dup() would preserve O_RDWR. Re-opening the memfd through procfs gives the
        // receiver a genuinely O_RDONLY descriptor, matching the Python v0.3 behavior.
        let path = format!("/proc/self/fd/{}", self.fd.as_raw_fd());
        let file = File::open(path)?;
        let raw = file.into_raw_fd();
        // SAFETY: into_raw_fd transfers ownership of this descriptor to us.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    pub fn client_mapping(&self, mode: AccessMode) -> Result<ClientMapping> {
        let fd = match mode {
            AccessMode::Read => self.duplicate_read_fd()?,
            AccessMode::Write => self.duplicate_write_fd()?,
        };
        Ok(ClientMapping {
            info: MappingInfo {
                backend: "memfd".into(),
                size: self.size,
                name: None,
                mode: None,
            },
            fd: Some(fd),
        })
    }
}
