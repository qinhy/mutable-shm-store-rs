# Security model

`mstore` is a local shared-memory service. Capability tokens are bearer secrets:
any process that obtains a valid token can exercise the permissions carried by
that token while it remains valid.

## Local endpoint

On Unix the default control endpoint is a Unix-domain socket under
`$XDG_RUNTIME_DIR` (or `/tmp`) and the socket mode is set to `0600`. Do not
replace this with a remotely reachable transport: Unix descriptor passing is
part of the security and data-plane design.

## Tokens

The daemon stores SHA-256 hashes of tokens rather than raw capability strings.
Keep raw tokens out of logs. Delegated capabilities cannot exceed the issuer's
permissions, and delegated expiry cannot outlive the issuer's expiry.

## Read mappings

On Linux a read mapping is handed out through an actual `O_RDONLY` descriptor,
not a duplicate of the daemon's `O_RDWR` descriptor. This prevents a holder of
only that descriptor from remapping it writable.

## Revocation boundary

Revocation prevents future opens. It cannot invalidate an mmap that another
process already established, and a process-local cached mapping intentionally
counts as already established. Deletion has the same boundary: it removes the
registry object and daemon backing reference, while live mappings continue until
their owners release them.

## Multiple writers

Write capabilities do not imply an exclusive writer lease. Applications must
coordinate writers. In the Rust API, zero-copy references are therefore exposed
through an explicit `unsafe` boundary until a cross-process lease protocol is
implemented. See [SAFETY.md](SAFETY.md).

## Trust assumptions

The design is intended for cooperating local processes, not as a sandbox against
root, kernel compromise, ptrace-capable peers, or arbitrary hostile code running
with equivalent OS privileges.
