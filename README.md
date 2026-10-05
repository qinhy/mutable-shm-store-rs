# mstore-rs

A small capability-based **mutable shared-memory object server** for large local
arrays and byte buffers, implemented in Rust and designed to remain wire-compatible
with the Python `mstore` v0.3 control protocol.

The main production target is **Linux / NVIDIA Jetson**.

```text
                       mstore daemon (Rust)
                     registry + capabilities
                              |
                  tiny JSON control messages
                              |
          +-------------------+-------------------+
          |                   |                   |
      camera service      inference service     Python tools
          |                   |                   |
          +-------------------+-------------------+
                              |
                    SAME shared-memory pages
                  memfd + mmap + SCM_RIGHTS
```

## Why

Do not send a 4K RGB image through the RPC serializer just to hand it to the
next local process. Put the pixels in one shared allocation and send a small
`object_id + token` instead.

## Status

`0.1.0` is a Linux/Jetson-first port of the Python v0.3 architecture. It includes:

- Linux anonymous `memfd` allocations
- Unix-domain control socket
- `SCM_RIGHTS` descriptor passing
- genuine O_RDONLY descriptors for read mappings
- persistent request/response control connections
- capability tokens with delegation, expiry and revocation
- server-owned lifetime
- deletion that preserves already-established mappings
- process-local LRU mapping cache
- raw byte access
- zero-copy `ndarray` views for primitive native-endian NumPy dtypes
- explicit `unsafe` boundary for direct shared-memory references while multiple writers are permitted
- the original 4-byte big-endian + compact JSON wire protocol
- macOS POSIX-shm backend code

The Rust Windows backend is intentionally not claimed complete yet. See
[MIGRATION.md](MIGRATION.md).

## Build

```bash
cargo build --release
cargo test --all-features
```

Run the daemon:

```bash
cargo run --release --bin mstore-server
# default: unix:///tmp/mstore-<uid>.sock (or XDG_RUNTIME_DIR)
```

Or choose an endpoint:

```bash
cargo run --release --bin mstore-server -- \
  --endpoint unix:///tmp/my-mstore.sock
```

## Rust API

```rust
use mstore::{connect, AccessMode, Result};
use serde_json::Map;

fn demo() -> Result<()> {
    let store = connect(None);
    let image = store.create_array::<u8>(&[2160, 3840, 3], "C", Map::new())?;

    unsafe { image.with_array_mut::<u8, _>(|mut arr| arr.fill(10)) }?;

    let read_token = image.issue("read", None)?;
    let reader = store.open(image.object_id(), &read_token, AccessMode::Read, true)?;

    unsafe {
        reader.with_array::<u8, _>(|arr| {
            println!("{:?} {}", arr.shape(), arr[[0, 0, 0]]);
        })
    }?;
    Ok(())
}
```

Raw buffers are equally direct:

```rust
let obj = store.create(1024 * 1024, None, None, "C", Map::new())?;
unsafe { obj.with_bytes_mut(|buf| buf[..4].copy_from_slice(b"MSTR")) }?;
```

## Python v0.3 compatibility

The Rust server intentionally accepts the current Python operations and framing.
With the original Python package importable:

```bash
cargo run --bin mstore-server -- --endpoint unix:///tmp/mstore-compat.sock
python scripts/python_compat_smoke.py unix:///tmp/mstore-compat.sock
```

The Python client can receive the Linux `memfd` from the Rust server and create a
NumPy array directly on the same pages.

## Data plane vs control plane

`mstore` is not an RPC framework and should not become one:

```text
RPC / NPB / tarpc       small calls, metadata, handles
          |
          v
       object_id
       capability
          |
          v
       mstore           large mutable local allocations
          |
          v
  ndarray / NumPy / TensorRT / camera pipeline
```

That separation is intentional.

## Capability model

Aliases match Python v0.3:

- `read` -> `read + info`
- `write` -> `read + write + info`
- `admin` -> `read + write + grant + delete + info`

The daemon stores SHA-256 hashes of capability tokens, not the raw tokens. A
grant-capable token cannot delegate permissions it does not already possess.

## Cache behavior

`open(..., cache=true)` first checks the process-local LRU. A hit performs no
control round trip, no server token hash lookup and no new mmap attachment.
Because a cached mapping is already established, later token expiry/revocation
does not invalidate it. Clear the cache when you want that local lease dropped.

## Synchronization

mstore intentionally permits multiple write capabilities to preserve the Python
behavior. This is not a writer-lock protocol. See [SAFETY.md](SAFETY.md). For
the capability and local-process threat model, see [SECURITY.md](SECURITY.md).

## Jetson recommendation

A practical production layout is:

```text
DepthAI -> Rust camera service -> mstore object
                                  |       |
                                  |       +-> recorder / PCD / SLAM
                                  +----------> TensorRT / CUDA inference

control/handles: NPB + your RPC layer
large buffers:   mstore
```

This keeps Python available for training/debugging while production camera and
inference services migrate to Rust incrementally.

## License

MIT
