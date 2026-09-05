// STUB: owned by agent 5 (asset). Fields and constants are frozen; the bodies are not.
//! The asset sidecar view, blob naming and the media-type table.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::FieldOrder;

/// Asset sidecar key order on disk and in JSON.
pub const ASSET_FIELDS: FieldOrder = FieldOrder(&[
    "id",
    "type",
    "title",
    "tags",
    "owner",
    "created",
    "updated",
    "related",
    "filename",
    "media_type",
    "bytes",
    "sha256",
    "blob",
]);

/// The fallback media type for an unknown extension.
pub const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

/// A validated asset sidecar.
#[derive(Clone, Debug)]
pub struct AssetSidecar {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub related: Vec<String>,
    pub filename: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
    pub blob: String,
    pub meta: Meta,
}

impl FromMeta for AssetSidecar {
    fn from_meta(_meta: &Meta) -> Option<AssetSidecar> {
        None
    }
}

impl Sortable for AssetSidecar {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.title.clone()),
            SortKey::Created => SortValue::Time(self.created),
            SortKey::Bytes => SortValue::Num(i64::try_from(self.bytes).unwrap_or(i64::MAX)),
            _ => SortValue::Time(self.updated),
        }
    }
}

/// The `status` payload's assets block.
#[derive(Clone, Copy, Debug, Default)]
pub struct AssetSummary {
    pub count: u64,
    pub bytes: u64,
    pub orphan_blobs: u64,
}

/// What `asset gc` found.
#[derive(Clone, Debug, Default)]
pub struct GcReport {
    pub orphan_blobs: Vec<String>,
    pub orphan_sidecars: Vec<String>,
    pub removed: u64,
}
