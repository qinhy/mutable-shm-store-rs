use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::backend::Region;
use crate::error::{MStoreError, Result};
use crate::models::{ObjectInfo, Permission, TokenInfo};

#[derive(Debug, Clone)]
pub struct ObjectRecord {
    pub info: ObjectInfo,
    pub region: Arc<Region>,
}

#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub object_id: String,
    pub permissions: BTreeSet<Permission>,
    pub generation: u64,
    pub created_at: f64,
    pub expires_at: Option<f64>,
    pub revoked: bool,
}

#[derive(Debug, Clone)]
pub struct Validated {
    pub object: ObjectRecord,
    pub token: TokenRecord,
}

#[derive(Default)]
struct RegistryInner {
    objects: HashMap<String, ObjectRecord>,
    tokens: HashMap<[u8; 32], TokenRecord>,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<RegistryInner>,
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn hash_token(raw: &str) -> [u8; 32] {
    Sha256::digest(raw.as_bytes()).into()
}

fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| MStoreError::Protocol(format!("secure randomness unavailable: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Normalize an iterable permission list using the Python v0.3 iterable semantics.
///
/// Notably, `admin` is an alias only when passed as a single string on the wire;
/// an array containing `"admin"` is rejected by the Python implementation too.
pub fn normalize_permissions<I, S>(values: I) -> Result<BTreeSet<Permission>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut result = BTreeSet::new();
    for value in values {
        let value = value.as_ref();
        let permission = Permission::parse(value)
            .ok_or_else(|| MStoreError::InvalidRequest(format!("unknown permission: {value:?}")))?;
        result.insert(permission);
    }
    if result.contains(&Permission::Write) {
        result.insert(Permission::Read);
    }
    if !result.is_empty() {
        result.insert(Permission::Info);
    }
    Ok(result)
}

/// Normalize a single string permission or public alias exactly like Python v0.3.
pub fn normalize_permission_alias(value: &str) -> Result<BTreeSet<Permission>> {
    match value {
        "read" => Ok([Permission::Read, Permission::Info].into_iter().collect()),
        "write" => Ok([Permission::Read, Permission::Write, Permission::Info]
            .into_iter()
            .collect()),
        "admin" => Ok(Permission::all()),
        other => normalize_permissions([other]),
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_object(
        &self,
        size: u64,
        shape: Option<Vec<usize>>,
        dtype: Option<String>,
        order: String,
        metadata: Map<String, Value>,
    ) -> Result<(ObjectRecord, String)> {
        if size == 0 {
            return Err(MStoreError::InvalidRequest("size must be > 0".into()));
        }
        let object_id = Uuid::new_v4().simple().to_string();
        let region = Arc::new(Region::create(size, &object_id)?);
        let info = ObjectInfo {
            object_id: object_id.clone(),
            size,
            generation: 1,
            shape,
            dtype,
            order,
            metadata,
            created_at: now_seconds(),
        };
        let record = ObjectRecord { info, region };

        let mut inner = self.inner.lock().expect("registry mutex poisoned");
        inner.objects.insert(object_id, record.clone());
        let token = Self::issue_locked(&mut inner, &record, Permission::all(), None)?;
        Ok((record, token))
    }

    fn issue_locked(
        inner: &mut RegistryInner,
        object: &ObjectRecord,
        permissions: BTreeSet<Permission>,
        expires_at: Option<f64>,
    ) -> Result<String> {
        let raw = new_token()?;
        let record = TokenRecord {
            object_id: object.info.object_id.clone(),
            permissions,
            generation: object.info.generation,
            created_at: now_seconds(),
            expires_at,
            revoked: false,
        };
        inner.tokens.insert(hash_token(&raw), record);
        Ok(raw)
    }

    fn validate_locked(
        inner: &RegistryInner,
        object_id: &str,
        raw_token: &str,
        required: Permission,
    ) -> Result<Validated> {
        if raw_token.is_empty() {
            return Err(MStoreError::Authentication("missing token".into()));
        }
        let object = inner.objects.get(object_id).ok_or_else(|| {
            MStoreError::ObjectNotFound(format!("object {object_id:?} does not exist"))
        })?;
        let token = inner
            .tokens
            .get(&hash_token(raw_token))
            .ok_or_else(|| MStoreError::Authentication("invalid token".into()))?;
        if token.object_id != object_id {
            return Err(MStoreError::Authentication("invalid token".into()));
        }
        if token.revoked {
            return Err(MStoreError::TokenRevoked("token has been revoked".into()));
        }
        if let Some(expires_at) = token.expires_at {
            if now_seconds() >= expires_at {
                return Err(MStoreError::TokenExpired("token has expired".into()));
            }
        }
        if token.generation != object.info.generation {
            return Err(MStoreError::Authentication(
                "token belongs to a stale object generation".into(),
            ));
        }
        if !token.permissions.contains(&required) {
            return Err(MStoreError::PermissionDenied(format!(
                "token lacks permission: {}",
                required.as_str()
            )));
        }
        Ok(Validated {
            object: object.clone(),
            token: token.clone(),
        })
    }

    pub fn validate(
        &self,
        object_id: &str,
        raw_token: &str,
        required: Permission,
    ) -> Result<Validated> {
        let inner = self.inner.lock().expect("registry mutex poisoned");
        Self::validate_locked(&inner, object_id, raw_token, required)
    }

    pub fn issue_token(
        &self,
        object_id: &str,
        issuer_raw: &str,
        permissions: BTreeSet<Permission>,
        expires_in: Option<f64>,
    ) -> Result<String> {
        let mut inner = self.inner.lock().expect("registry mutex poisoned");
        let validated = Self::validate_locked(&inner, object_id, issuer_raw, Permission::Grant)?;
        if !permissions.is_subset(&validated.token.permissions) {
            return Err(MStoreError::PermissionDenied(
                "cannot delegate permissions the issuer does not possess".into(),
            ));
        }
        let expires_at = match expires_in {
            Some(seconds) if seconds <= 0.0 => {
                return Err(MStoreError::InvalidRequest("expires_in must be > 0".into()))
            }
            Some(seconds) => {
                let requested = now_seconds() + seconds;
                Some(
                    validated
                        .token
                        .expires_at
                        .map_or(requested, |issuer| requested.min(issuer)),
                )
            }
            None => validated.token.expires_at,
        };
        Self::issue_locked(&mut inner, &validated.object, permissions, expires_at)
    }

    pub fn revoke_token(&self, object_id: &str, issuer_raw: &str, target_raw: &str) -> Result<()> {
        let mut inner = self.inner.lock().expect("registry mutex poisoned");
        let _ = Self::validate_locked(&inner, object_id, issuer_raw, Permission::Grant)?;
        let target_hash = hash_token(target_raw);
        let target = inner
            .tokens
            .get_mut(&target_hash)
            .ok_or_else(|| MStoreError::Authentication("target token is invalid".into()))?;
        if target.object_id != object_id {
            return Err(MStoreError::Authentication(
                "target token is invalid".into(),
            ));
        }
        if target.permissions == Permission::all() && target_raw == issuer_raw {
            return Err(MStoreError::InvalidRequest(
                "refusing to revoke the token currently authorizing this request".into(),
            ));
        }
        target.revoked = true;
        Ok(())
    }

    pub fn delete_object(&self, object_id: &str, raw_token: &str) -> Result<()> {
        let removed = {
            let mut inner = self.inner.lock().expect("registry mutex poisoned");
            let _ = Self::validate_locked(&inner, object_id, raw_token, Permission::Delete)?;
            let record = inner.objects.remove(object_id);
            inner.tokens.retain(|_, token| token.object_id != object_id);
            record
        };
        drop(removed);
        Ok(())
    }

    pub fn token_info(token: &TokenRecord) -> TokenInfo {
        TokenInfo {
            permissions: token
                .permissions
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect(),
            expires_at: token.expires_at,
        }
    }
}
