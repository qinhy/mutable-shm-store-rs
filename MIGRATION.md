# Migration from Python mstore v0.3

The recommended migration is deliberately incremental.

1. Keep the existing Python package installed.
2. Start the Rust daemon with the same Unix endpoint.
3. Run `scripts/python_compat_smoke.py` against it.
4. Move individual producers/consumers to the Rust `Client` API.
5. Keep Python NumPy tools attached to the same objects while Rust services are
   introduced.

The first Rust release targets Jetson/Linux. The control framing, operation
names, capability semantics, NumPy dtype metadata, read-only Linux descriptor
behavior, deletion semantics, and cached-mapping semantics are designed to match
Python v0.3.

macOS POSIX shared memory is implemented but should be validated on macOS CI
before production deployment. The current Rust crate does not implement the
Python `multiprocessing.connection` named-pipe framing used by the Windows v0.3
control path; keep the Python implementation on Windows until that backend is
ported and interoperability-tested.
