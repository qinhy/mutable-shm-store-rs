# mstore wire protocol

The Rust implementation intentionally keeps the Python v0.3 control protocol.

## Framing

Each control message is:

```text
+----------------------+----------------------+
| u32 big-endian bytes | compact UTF-8 JSON   |
+----------------------+----------------------+
```

The JSON payload is limited to 4 MiB. The large shared-memory payload is never
embedded in this frame.

## Request

```json
{"op":"open","args":{"object_id":"...","token":"...","mode":"read"}}
```

Operations: `ping`, `create`, `open`, `info`, `grant`, `revoke`, `delete`.

## Success response

```json
{"ok":true,"result":{}}
```

For `create` and `open` on Unix, the response frame may carry exactly one
`SCM_RIGHTS` file descriptor.

## Error response

```json
{"ok":false,"error":{"type":"permission_denied","message":"..."}}
```

Stable error type strings are:

- `authentication_error`
- `permission_denied`
- `object_not_found`
- `token_expired`
- `token_revoked`
- `invalid_request`
- `protocol_error`
- `internal_error`

## Linux mapping semantics

The daemon owns an anonymous `memfd`. Write clients receive a duplicated O_RDWR
file descriptor. Read clients receive a descriptor re-opened through
`/proc/self/fd/<N>` as O_RDONLY, so a read capability cannot simply remap the
received descriptor writable.
