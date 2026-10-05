#![cfg(target_os = "linux")]

use std::os::fd::AsRawFd;

use mstore::backend::Region;
use mstore::AccessMode;

#[test]
fn read_capability_receives_a_genuinely_read_only_fd() {
    let region = Region::create(4096, "readonly-test").unwrap();
    let mapping = region.client_mapping(AccessMode::Read).unwrap();
    let fd = mapping.fd.unwrap();

    // A read mapping succeeds.
    let read_ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    assert_ne!(read_ptr, libc::MAP_FAILED);
    unsafe { libc::munmap(read_ptr, 4096) };

    // A writable mapping through that same descriptor must fail.
    let write_ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    assert_eq!(write_ptr, libc::MAP_FAILED);
}
