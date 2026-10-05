# Changelog

## 0.1.0

Initial Linux/Jetson-first Rust port of Python `mstore` v0.3 semantics:

- `memfd` + `SCM_RIGHTS` zero-copy handoff on Linux
- persistent Unix-domain control connections
- compatible length-prefixed JSON control protocol
- capability issue/delegation/expiry/revocation
- server-owned object lifetime and deletion semantics
- process-local LRU mapping cache
- raw byte and `ndarray` zero-copy views
- explicit Rust safety boundary for externally mutable mappings
- macOS POSIX-shm backend implementation
- Python v0.3 golden protocol vectors and interoperability smoke script
