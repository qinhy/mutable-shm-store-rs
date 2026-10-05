use std::num::NonZeroUsize;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lru::LruCache;
use serde_json::{json, Map, Value};

use crate::error::{MStoreError, Result};
use crate::models::{AccessMode, MappingInfo, ObjectInfo};
use crate::server::default_endpoint;
use crate::transport::{connect_control, ControlConnection};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    object_id: String,
    token: String,
    mode: AccessMode,
}

#[derive(Debug)]
struct RawMapping {
    ptr: std::ptr::NonNull<u8>,
    len: usize,
}

// RawMapping owns an OS mapping. It exposes no safe references; callers must enter
// the explicit unsafe access boundary below, so moving/sharing the handle is sound.
unsafe impl Send for RawMapping {}
unsafe impl Sync for RawMapping {}

impl RawMapping {
    fn map(fd: &OwnedFd, len: usize, mode: AccessMode) -> Result<Self> {
        if len == 0 {
            return Err(MStoreError::Protocol("cannot map zero bytes".into()));
        }
        let prot = match mode {
            AccessMode::Read => libc::PROT_READ,
            AccessMode::Write => libc::PROT_READ | libc::PROT_WRITE,
        };
        // SAFETY: fd is a valid shared-memory descriptor, len is non-zero, offset is
        // page-aligned zero, and MAP_SHARED is valid for this descriptor. No Rust
        // reference is created here; aliasing is handled at the explicit access boundary.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let Some(ptr) = std::ptr::NonNull::new(raw.cast::<u8>()) else {
            // SAFETY: the mapping succeeded above and must be released on this rare path.
            unsafe { libc::munmap(raw, len) };
            return Err(MStoreError::Protocol(
                "mmap unexpectedly returned a null pointer".into(),
            ));
        };
        Ok(Self { ptr, len })
    }

    unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: guaranteed by the caller of MappingState::with_bytes.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: guaranteed by the caller of MappingState::with_bytes_mut.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for RawMapping {
    fn drop(&mut self) {
        // SAFETY: ptr/len came from one successful mmap and are unmapped exactly once.
        unsafe {
            libc::munmap(self.ptr.as_ptr().cast::<libc::c_void>(), self.len);
        }
    }
}

#[derive(Debug)]
enum Mapping {
    Read(RawMapping),
    Write(Mutex<RawMapping>),
}

#[derive(Debug)]
struct MappingState {
    mapping: Mapping,
    info: ObjectInfo,
}

impl MappingState {
    unsafe fn with_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        match &self.mapping {
            Mapping::Read(mapping) => {
                // SAFETY: forwarded from the caller.
                f(unsafe { mapping.as_slice() })
            }
            Mapping::Write(mapping) => {
                let guard = mapping.lock().expect("mapping mutex poisoned");
                // SAFETY: forwarded from the caller; this process-local mutex prevents
                // simultaneous mutable access through this cached Rust mapping.
                f(unsafe { guard.as_slice() })
            }
        }
    }

    unsafe fn with_bytes_mut<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> Result<R> {
        match &self.mapping {
            Mapping::Read(_) => Err(MStoreError::PermissionDenied(
                "shared object is mapped read-only".into(),
            )),
            Mapping::Write(mapping) => {
                let mut guard = mapping.lock().expect("mapping mutex poisoned");
                // SAFETY: forwarded from the caller and serialized in this process.
                Ok(f(unsafe { guard.as_mut_slice() }))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheInfo {
    pub size: usize,
    pub capacity: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

struct ClientInner {
    endpoint: String,
    timeout: Duration,
    connection: Option<ControlConnection>,
    cache: Option<LruCache<CacheKey, Arc<MappingState>>>,
    cache_capacity: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
    pid: u32,
    closed: bool,
}

pub struct Client {
    inner: Mutex<ClientInner>,
}

pub struct SharedObject {
    client: Arc<Client>,
    info: ObjectInfo,
    token: String,
    mode: AccessMode,
    state: Arc<MappingState>,
    cache_hit: bool,
}

impl SharedObject {
    pub fn object_id(&self) -> &str {
        &self.info.object_id
    }
    pub fn size(&self) -> u64 {
        self.info.size
    }
    pub fn shape(&self) -> Option<&[usize]> {
        self.info.shape.as_deref()
    }
    pub fn dtype(&self) -> Option<&str> {
        self.info.dtype.as_deref()
    }
    pub fn order(&self) -> &str {
        &self.info.order
    }
    pub fn metadata(&self) -> &Map<String, Value> {
        &self.info.metadata
    }
    pub fn generation(&self) -> u64 {
        self.info.generation
    }
    pub fn mode(&self) -> AccessMode {
        self.mode
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn cache_hit(&self) -> bool {
        self.cache_hit
    }

    /// Borrow the mapped bytes for the duration of `f` without copying.
    ///
    /// # Safety
    /// The caller must ensure that no unsynchronized writer in this or another
    /// process mutates the same bytes while `f` holds the shared-memory reference.
    pub unsafe fn with_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        // SAFETY: the caller accepts the cross-process synchronization contract.
        unsafe { self.state.with_bytes(f) }
    }

    /// Mutably borrow the mapped bytes for the duration of `f` without copying.
    ///
    /// # Safety
    /// The caller must ensure exclusive access to the touched bytes across all
    /// processes for the lifetime of the closure invocation. The internal mutex
    /// only serializes access through this process-local cached mapping.
    pub unsafe fn with_bytes_mut<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> Result<R> {
        // SAFETY: the caller accepts the cross-process synchronization contract.
        unsafe { self.state.with_bytes_mut(f) }
    }

    /// Issue a capability using the Python-compatible single-string aliases
    /// (`read`, `write`, or `admin`).
    pub fn issue(&self, permissions: &str, expires_in: Option<f64>) -> Result<String> {
        self.client
            .issue_token(self.object_id(), &self.token, permissions, expires_in)
    }

    /// Issue a capability from an explicit permission list.
    ///
    /// This follows Python v0.3 iterable semantics: `admin` is not a list item;
    /// use [`SharedObject::issue`] with `"admin"` for that alias.
    pub fn issue_many(&self, permissions: &[&str], expires_in: Option<f64>) -> Result<String> {
        self.client
            .issue_token_many(self.object_id(), &self.token, permissions, expires_in)
    }

    pub fn revoke(&self, target_token: &str) -> Result<()> {
        self.client
            .revoke_token(self.object_id(), &self.token, target_token)
    }

    pub fn info(&self) -> Result<Value> {
        self.client.info(self.object_id(), &self.token)
    }

    pub fn delete(&self) -> Result<()> {
        self.client.delete(self.object_id(), &self.token)
    }
}

impl Client {
    pub fn new(endpoint: Option<String>, timeout: Duration, cache_size: usize) -> Arc<Self> {
        let cache = NonZeroUsize::new(cache_size).map(LruCache::new);
        Arc::new(Self {
            inner: Mutex::new(ClientInner {
                endpoint: endpoint.unwrap_or_else(default_endpoint),
                timeout,
                connection: None,
                cache,
                cache_capacity: cache_size,
                hits: 0,
                misses: 0,
                evictions: 0,
                pid: std::process::id(),
                closed: false,
            }),
        })
    }

    fn after_fork_if_needed(inner: &mut ClientInner) {
        let pid = std::process::id();
        if pid == inner.pid {
            return;
        }
        inner.connection = None;
        if let Some(cache) = &mut inner.cache {
            cache.clear();
        }
        inner.hits = 0;
        inner.misses = 0;
        inner.evictions = 0;
        inner.pid = pid;
        inner.closed = false;
    }

    fn request_locked(
        inner: &mut ClientInner,
        op: &str,
        args: Value,
    ) -> Result<(Value, Option<OwnedFd>)> {
        Self::after_fork_if_needed(inner);
        if inner.closed {
            return Err(MStoreError::Protocol("client is closed".into()));
        }
        if inner.connection.is_none() {
            inner.connection = Some(connect_control(&inner.endpoint, inner.timeout)?);
        }
        let request = json!({"op": op, "args": args});
        let send_result = inner.connection.as_mut().unwrap().send(&request, None);
        if let Err(err) = send_result {
            inner.connection = None;
            return Err(MStoreError::Protocol(format!(
                "mstore control connection failed during {op:?}: {err}"
            )));
        }
        let recv_result = inner.connection.as_mut().unwrap().recv();
        let (response, fd) = match recv_result {
            Ok(value) => value,
            Err(err) => {
                inner.connection = None;
                return Err(MStoreError::Protocol(format!(
                    "mstore control connection failed during {op:?}: {err}"
                )));
            }
        };
        let response = response.as_object().ok_or_else(|| {
            MStoreError::Protocol("server response must be a JSON object".into())
        })?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            drop(fd);
            let error = response.get("error").and_then(Value::as_object);
            let code = error
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("internal_error");
            let message = error
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or(code)
                .to_owned();
            return Err(MStoreError::from_remote(code, message));
        }
        let result = response.get("result").cloned().ok_or_else(|| {
            MStoreError::Protocol("server response is missing a result object".into())
        })?;
        if !result.is_object() {
            return Err(MStoreError::Protocol(
                "server response result must be an object".into(),
            ));
        }
        Ok((result, fd))
    }

    fn open_mapping(
        mapping: &MappingInfo,
        fd: Option<OwnedFd>,
        mode: AccessMode,
    ) -> Result<Mapping> {
        match mapping.backend.as_str() {
            "memfd" | "posix_shm" => {
                let fd = fd.ok_or_else(|| {
                    MStoreError::Protocol(format!(
                        "{} response did not include a file descriptor",
                        mapping.backend
                    ))
                })?;
                let len = usize::try_from(mapping.size).map_err(|_| {
                    MStoreError::Protocol("mapping is too large for this process".into())
                })?;
                let raw = RawMapping::map(&fd, len, mode)?;
                match mode {
                    AccessMode::Read => Ok(Mapping::Read(raw)),
                    AccessMode::Write => Ok(Mapping::Write(Mutex::new(raw))),
                }
            }
            other => {
                drop(fd);
                Err(MStoreError::Protocol(format!(
                    "unsupported mapping backend: {other:?}"
                )))
            }
        }
    }

    pub fn ping(&self) -> Result<Value> {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(&mut inner, "ping", json!({}))?;
        drop(fd);
        Ok(result)
    }

    pub fn create(
        self: &Arc<Self>,
        size: u64,
        shape: Option<Vec<usize>>,
        dtype: Option<String>,
        order: &str,
        metadata: Map<String, Value>,
    ) -> Result<SharedObject> {
        if size == 0 {
            return Err(MStoreError::InvalidRequest("size must be > 0".into()));
        }
        if order != "C" && order != "F" {
            return Err(MStoreError::InvalidRequest(
                "order must be 'C' or 'F'".into(),
            ));
        }
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(
            &mut inner,
            "create",
            json!({
                "size": size,
                "shape": shape,
                "dtype": dtype,
                "order": order,
                "metadata": metadata,
            }),
        )?;
        let token = result
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| MStoreError::Protocol("create response is missing token".into()))?
            .to_owned();
        let info: ObjectInfo = serde_json::from_value(
            result
                .get("object")
                .cloned()
                .ok_or_else(|| MStoreError::Protocol("create response is missing object".into()))?,
        )?;
        let mapping: MappingInfo = serde_json::from_value(
            result
                .get("mapping")
                .cloned()
                .ok_or_else(|| MStoreError::Protocol("create response is missing mapping".into()))?,
        )?;
        let state = Arc::new(MappingState {
            mapping: Self::open_mapping(&mapping, fd, AccessMode::Write)?,
            info: info.clone(),
        });
        Ok(SharedObject {
            client: Arc::clone(self),
            info,
            token,
            mode: AccessMode::Write,
            state,
            cache_hit: false,
        })
    }

    pub fn open(
        self: &Arc<Self>,
        object_id: &str,
        token: &str,
        mode: AccessMode,
        cache_enabled: bool,
    ) -> Result<SharedObject> {
        let key = CacheKey {
            object_id: object_id.to_owned(),
            token: token.to_owned(),
            mode,
        };
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        Self::after_fork_if_needed(&mut inner);

        if cache_enabled {
            let cached = inner
                .cache
                .as_mut()
                .and_then(|cache| cache.get(&key).cloned());
            if let Some(state) = cached {
                inner.hits += 1;
                return Ok(SharedObject {
                    client: Arc::clone(self),
                    info: state.info.clone(),
                    token: token.to_owned(),
                    mode,
                    state,
                    cache_hit: true,
                });
            }
            inner.misses += 1;
        }

        let (result, fd) = Self::request_locked(
            &mut inner,
            "open",
            json!({"object_id": object_id, "token": token, "mode": mode.as_str()}),
        )?;
        let info: ObjectInfo = serde_json::from_value(
            result
                .get("object")
                .cloned()
                .ok_or_else(|| MStoreError::Protocol("open response is missing object".into()))?,
        )?;
        let mapping: MappingInfo = serde_json::from_value(
            result
                .get("mapping")
                .cloned()
                .ok_or_else(|| MStoreError::Protocol("open response is missing mapping".into()))?,
        )?;
        let state = Arc::new(MappingState {
            mapping: Self::open_mapping(&mapping, fd, mode)?,
            info: info.clone(),
        });

        if cache_enabled && inner.cache_capacity > 0 {
            let capacity = inner.cache_capacity;
            let will_evict = inner
                .cache
                .as_ref()
                .is_some_and(|cache| cache.len() == capacity);
            if will_evict {
                inner.evictions += 1;
            }
            if let Some(cache) = &mut inner.cache {
                cache.put(key, Arc::clone(&state));
            }
        }

        Ok(SharedObject {
            client: Arc::clone(self),
            info,
            token: token.to_owned(),
            mode,
            state,
            cache_hit: false,
        })
    }

    fn issue_token_value(
        &self,
        object_id: &str,
        issuer_token: &str,
        permissions: Value,
        expires_in: Option<f64>,
    ) -> Result<String> {
        let mut args = json!({
            "object_id": object_id,
            "token": issuer_token,
            "permissions": permissions,
        });
        if let Some(value) = expires_in {
            args["expires_in"] = json!(value);
        }
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(&mut inner, "grant", args)?;
        drop(fd);
        result
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| MStoreError::Protocol("grant response is missing token".into()))
    }

    pub fn issue_token(
        &self,
        object_id: &str,
        issuer_token: &str,
        permissions: &str,
        expires_in: Option<f64>,
    ) -> Result<String> {
        self.issue_token_value(
            object_id,
            issuer_token,
            Value::String(permissions.to_owned()),
            expires_in,
        )
    }

    pub fn issue_token_many(
        &self,
        object_id: &str,
        issuer_token: &str,
        permissions: &[&str],
        expires_in: Option<f64>,
    ) -> Result<String> {
        self.issue_token_value(
            object_id,
            issuer_token,
            Value::Array(
                permissions
                    .iter()
                    .map(|permission| Value::String((*permission).to_owned()))
                    .collect(),
            ),
            expires_in,
        )
    }

    pub fn revoke_token(
        &self,
        object_id: &str,
        issuer_token: &str,
        target_token: &str,
    ) -> Result<()> {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(
            &mut inner,
            "revoke",
            json!({
                "object_id": object_id,
                "token": issuer_token,
                "target_token": target_token,
            }),
        )?;
        drop(fd);
        if result.get("revoked").and_then(Value::as_bool) != Some(true) {
            return Err(MStoreError::Protocol(
                "server did not confirm token revocation".into(),
            ));
        }
        Ok(())
    }

    pub fn info(&self, object_id: &str, token: &str) -> Result<Value> {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(
            &mut inner,
            "info",
            json!({"object_id": object_id, "token": token}),
        )?;
        drop(fd);
        Ok(result)
    }

    pub fn delete(&self, object_id: &str, token: &str) -> Result<()> {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let (result, fd) = Self::request_locked(
            &mut inner,
            "delete",
            json!({"object_id": object_id, "token": token}),
        )?;
        drop(fd);
        if result.get("deleted").and_then(Value::as_bool) != Some(true) {
            return Err(MStoreError::Protocol(
                "server did not confirm object deletion".into(),
            ));
        }
        if let Some(cache) = &mut inner.cache {
            let keys: Vec<_> = cache
                .iter()
                .filter_map(|(key, _)| (key.object_id == object_id).then(|| key.clone()))
                .collect();
            for key in keys {
                cache.pop(&key);
            }
        }
        Ok(())
    }

    pub fn clear_cache(&self, object_id: Option<&str>) -> usize {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        let Some(cache) = &mut inner.cache else {
            return 0;
        };
        if object_id.is_none() {
            let count = cache.len();
            cache.clear();
            return count;
        }
        let object_id = object_id.unwrap();
        let keys: Vec<_> = cache
            .iter()
            .filter_map(|(key, _)| (key.object_id == object_id).then(|| key.clone()))
            .collect();
        let count = keys.len();
        for key in keys {
            cache.pop(&key);
        }
        count
    }

    pub fn cache_info(&self) -> CacheInfo {
        let inner = self.inner.lock().expect("client mutex poisoned");
        CacheInfo {
            size: inner.cache.as_ref().map_or(0, |cache| cache.len()),
            capacity: inner.cache_capacity,
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
        }
    }

    pub fn close(&self) {
        let mut inner = self.inner.lock().expect("client mutex poisoned");
        if let Some(cache) = &mut inner.cache {
            cache.clear();
        }
        inner.connection = None;
        inner.closed = true;
    }
}

pub fn connect(endpoint: Option<String>) -> Arc<Client> {
    Client::new(endpoint, Duration::from_secs(10), 64)
}

#[cfg(feature = "ndarray")]
impl Client {
    pub fn create_array<T: crate::array::ShmElement>(
        self: &Arc<Self>,
        shape: &[usize],
        order: &str,
        metadata: Map<String, Value>,
    ) -> Result<SharedObject> {
        if shape.is_empty() || shape.contains(&0) {
            return Err(MStoreError::InvalidRequest(
                "shape dimensions must be positive".into(),
            ));
        }
        let count = shape.iter().try_fold(1usize, |acc, &x| acc.checked_mul(x)).ok_or_else(|| {
            MStoreError::InvalidRequest("array element count overflow".into())
        })?;
        let size = count.checked_mul(std::mem::size_of::<T>()).ok_or_else(|| {
            MStoreError::InvalidRequest("array byte size overflow".into())
        })?;
        self.create(
            size as u64,
            Some(shape.to_vec()),
            Some(T::numpy_dtype()),
            order,
            metadata,
        )
    }
}
