use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::error::{MStoreError, Result};
use crate::models::{AccessMode, Permission};
use crate::registry::{normalize_permission_alias, normalize_permissions, Registry};
use crate::transport::{parse_endpoint, ControlConnection, Endpoint, Listener};

pub fn default_endpoint() -> String {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    format!("unix://{runtime}/mstore-{uid}.sock")
}

pub struct MStoreServer {
    endpoint: String,
    debug: bool,
    pub registry: Arc<Registry>,
    stop: AtomicBool,
}

impl MStoreServer {
    pub fn new(endpoint: Option<String>, debug: bool) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(default_endpoint);
        if matches!(parse_endpoint(&endpoint)?, Endpoint::Tcp(_)) {
            return Err(MStoreError::InvalidRequest(
                "tcp:// control endpoints cannot carry shared-memory descriptors; use unix:// on Linux/macOS"
                    .into(),
            ));
        }
        Ok(Self {
            endpoint,
            debug,
            registry: Arc::new(Registry::new()),
            stop: AtomicBool::new(false),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn serve_forever(self: &Arc<Self>) -> Result<()> {
        let (listener, published) = Listener::bind(&self.endpoint)?;
        if published != self.endpoint {
            return Err(MStoreError::Protocol(format!(
                "published endpoint changed unexpectedly: {published}"
            )));
        }

        while !self.stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok(conn) => {
                    let server = Arc::clone(self);
                    thread::spawn(move || server.serve_connection(conn));
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) if self.stop.load(Ordering::Acquire) => break,
                Err(err) => {
                    listener.cleanup();
                    return Err(err.into());
                }
            }
        }
        listener.cleanup();
        Ok(())
    }

    fn serve_connection(&self, mut conn: ControlConnection) {
        loop {
            if self.stop.load(Ordering::Acquire) {
                break;
            }
            let (request, request_fd) = match conn.recv() {
                Ok(value) => value,
                Err(err @ MStoreError::Protocol(_)) => {
                    let _ = conn.send(&self.error_payload(&err), None);
                    break;
                }
                Err(_) => break,
            };
            if request_fd.is_some() {
                let response = self.error_payload(&MStoreError::Protocol(
                    "unexpected client file descriptor".into(),
                ));
                let _ = conn.send(&response, None);
                break;
            }
            match self.handle(&request) {
                Ok((result, fd)) => {
                    let response = json!({"ok": true, "result": result});
                    if conn.send(&response, fd.as_ref()).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let response = self.error_payload(&err);
                    if conn.send(&response, None).is_err() {
                        break;
                    }
                }
            }
        }
    }

    fn error_payload(&self, err: &MStoreError) -> Value {
        let mut error = Map::new();
        error.insert("type".into(), Value::String(err.protocol_code().into()));
        error.insert("message".into(), Value::String(err.to_string()));
        if self.debug && err.protocol_code() == "internal_error" {
            error.insert("debug".into(), Value::String(format!("{err:?}")));
        }
        json!({"ok": false, "error": error})
    }

    fn handle(&self, request: &Value) -> Result<(Value, Option<std::os::fd::OwnedFd>)> {
        let object = request
            .as_object()
            .ok_or_else(|| MStoreError::InvalidRequest("request must be a JSON object".into()))?;
        let op = object.get("op").and_then(Value::as_str).unwrap_or("");
        let args = object.get("args").cloned().unwrap_or_else(|| json!({}));
        let args = args
            .as_object()
            .ok_or_else(|| MStoreError::InvalidRequest("args must be an object".into()))?;

        if op == "ping" {
            return Ok((json!({"pong": true, "endpoint": self.endpoint}), None));
        }

        if op == "create" {
            let size = args.get("size").and_then(Value::as_u64).ok_or_else(|| {
                MStoreError::InvalidRequest("size is required and must be an integer".into())
            })?;
            let shape = match args.get("shape") {
                Some(Value::Array(values)) => Some(
                    values
                        .iter()
                        .map(|v| {
                            v.as_u64()
                                .and_then(|x| usize::try_from(x).ok())
                                .ok_or_else(|| {
                                    MStoreError::InvalidRequest(
                                        "shape must contain non-negative integers".into(),
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>>>()?,
                ),
                Some(Value::Null) | None => None,
                _ => {
                    return Err(MStoreError::InvalidRequest(
                        "shape must be an array or null".into(),
                    ))
                }
            };
            let dtype = args.get("dtype").and_then(Value::as_str).map(str::to_owned);
            let order = args
                .get("order")
                .and_then(Value::as_str)
                .unwrap_or("C")
                .to_owned();
            let metadata = match args.get("metadata") {
                Some(Value::Object(map)) => map.clone(),
                Some(Value::Null) | None => Map::new(),
                _ => {
                    return Err(MStoreError::InvalidRequest(
                        "metadata must be an object".into(),
                    ))
                }
            };
            let (record, token) = self
                .registry
                .create_object(size, shape, dtype, order, metadata)?;
            let mapping = record.region.client_mapping(AccessMode::Write)?;
            return Ok((
                json!({
                    "object": record.info,
                    "token": token,
                    "mapping": mapping.info,
                }),
                mapping.fd,
            ));
        }

        let object_id = args.get("object_id").and_then(Value::as_str).unwrap_or("");
        let token = args.get("token").and_then(Value::as_str).unwrap_or("");
        if object_id.is_empty() {
            return Err(MStoreError::InvalidRequest("object_id is required".into()));
        }

        match op {
            "open" => {
                let mode_str = args.get("mode").and_then(Value::as_str).unwrap_or("read");
                let mode = AccessMode::parse(mode_str).ok_or_else(|| {
                    MStoreError::InvalidRequest("mode must be 'read' or 'write'".into())
                })?;
                let required = match mode {
                    AccessMode::Read => Permission::Read,
                    AccessMode::Write => Permission::Write,
                };
                let validated = self.registry.validate(object_id, token, required)?;
                let mapping = validated.object.region.client_mapping(mode)?;
                Ok((
                    json!({"object": validated.object.info, "mapping": mapping.info}),
                    mapping.fd,
                ))
            }
            "info" => {
                let validated = self.registry.validate(object_id, token, Permission::Info)?;
                Ok((
                    json!({
                        "object": validated.object.info,
                        "token": Registry::token_info(&validated.token),
                    }),
                    None,
                ))
            }
            "grant" => {
                let permissions = match args.get("permissions") {
                    Some(Value::String(value)) => normalize_permission_alias(value)?,
                    Some(Value::Array(values)) => {
                        let values = values
                            .iter()
                            .map(|v| {
                                v.as_str().map(str::to_owned).ok_or_else(|| {
                                    MStoreError::InvalidRequest(
                                        "permissions must contain strings".into(),
                                    )
                                })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        normalize_permissions(&values)?
                    }
                    Some(_) => {
                        return Err(MStoreError::InvalidRequest(
                            "permissions must be a string or string array".into(),
                        ))
                    }
                    None => normalize_permission_alias("read")?,
                };
                let expires_in = args.get("expires_in").and_then(Value::as_f64);
                let issued =
                    self.registry
                        .issue_token(object_id, token, permissions, expires_in)?;
                Ok((json!({"token": issued}), None))
            }
            "revoke" => {
                let target = args
                    .get("target_token")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if target.is_empty() {
                    return Err(MStoreError::InvalidRequest(
                        "target_token is required".into(),
                    ));
                }
                self.registry.revoke_token(object_id, token, target)?;
                Ok((json!({"revoked": true}), None))
            }
            "delete" => {
                self.registry.delete_object(object_id, token)?;
                Ok((json!({"deleted": true}), None))
            }
            _ => Err(MStoreError::InvalidRequest(format!(
                "unknown operation: {op:?}"
            ))),
        }
    }
}
