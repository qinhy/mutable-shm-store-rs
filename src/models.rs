use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessMode {
    Read,
    Write,
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    Delete,
    Grant,
    Info,
    Read,
    Write,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Grant => "grant",
            Self::Info => "info",
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "delete" => Some(Self::Delete),
            "grant" => Some(Self::Grant),
            "info" => Some(Self::Info),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    pub fn all() -> BTreeSet<Self> {
        [
            Self::Read,
            Self::Write,
            Self::Grant,
            Self::Delete,
            Self::Info,
        ]
        .into_iter()
        .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectInfo {
    pub object_id: String,
    pub size: u64,
    pub generation: u64,
    pub shape: Option<Vec<usize>>,
    pub dtype: Option<String>,
    pub order: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MappingInfo {
    pub backend: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenInfo {
    pub permissions: Vec<String>,
    pub expires_at: Option<f64>,
}
