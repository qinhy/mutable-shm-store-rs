# Safety and synchronization

mstore is intentionally a **mutable** shared-memory store. Capability checks
control whether a client may establish a mapping. They are not a distributed
reader/writer lock.

## Existing mappings

Revoking a token prevents future opens. It does not invalidate mappings that a
process already established. Deleting an object removes it from the registry and
drops the daemon's backing reference, while existing mappings remain valid until
their owners close them.

## Multiple writers

Multiple write tokens are allowed for Python v0.3 compatibility. Concurrent
writers touching the same bytes require application-level synchronization.
`with_bytes_mut` serializes mutable access only among Rust handles sharing the
same process-local cached mapping; it cannot lock another process.

## ndarray views

The ndarray helpers validate shape, byte size, primitive dtype and native
endianness before creating a zero-copy view. The view is closure-scoped so it
cannot outlive its mapping guard. This does not replace cross-process
synchronization.

A future lease protocol can add server-mediated read leases / exclusive write
leases without changing the shared-memory data plane.

## Rust unsafe boundary

Direct zero-copy reference access (`with_bytes`, `with_bytes_mut`, `with_array`,
`with_array_mut`) is intentionally `unsafe` in v0.1. Python v0.3 permits multiple
independent writers, so Rust cannot prove the aliasing/synchronization guarantees
required by `&[T]` / `&mut [T]`. The planned lease protocol is the path to safe
typed views: safe read views under a read lease and safe mutable views under an
exclusive write lease.
