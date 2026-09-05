//! The asset sidecar view, blob naming and the media-type table.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::{Meta, Value};
use crate::model::common::{meta_int, meta_str, meta_time, FieldOrder};

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

/// The id prefix every asset carries; the sidecar's filename stem is the id.
pub const ASSET_ID_PREFIX: &str = "a-";

/// The literal `type` every asset sidecar carries.
pub const ASSET_TYPE: &str = "asset";

/// The longest extension a blob filename may carry.
pub const MAX_EXTENSION_CHARS: usize = 12;

/// Extension → media type. Anything not here is [`DEFAULT_MEDIA_TYPE`].
pub const MEDIA_TYPES: [(&str, &str); 34] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("ico", "image/vnd.microsoft.icon"),
    ("heic", "image/heic"),
    ("avif", "image/avif"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("html", "text/html"),
    ("css", "text/css"),
    ("json", "application/json"),
    ("yaml", "application/yaml"),
    ("yml", "application/yaml"),
    ("toml", "application/toml"),
    ("xml", "application/xml"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("tar", "application/x-tar"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("m4a", "audio/mp4"),
    ("ogg", "audio/ogg"),
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
];

/// The media type an extension denotes, or [`DEFAULT_MEDIA_TYPE`].
pub fn media_type_for(ext: Option<&str>) -> &'static str {
    let Some(ext) = ext else {
        return DEFAULT_MEDIA_TYPE;
    };
    MEDIA_TYPES
        .iter()
        .find(|(name, _)| *name == ext)
        .map_or(DEFAULT_MEDIA_TYPE, |(_, media)| *media)
}

/// The extension a blob keeps: the source's own, lowercased, only when it matches
/// `[a-z0-9]{1,12}` after lowercasing. Anything else — a dotfile, a hostile name, an
/// extension carrying punctuation or a very long one — gets no extension at all.
pub fn blob_extension(filename: &str) -> Option<String> {
    let (stem, ext) = filename.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.chars().count() > MAX_EXTENSION_CHARS {
        return None;
    }
    if !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// The blob's filename: `<id>` plus the kept extension, relative to the assets root.
pub fn blob_name(id: &str, ext: Option<&str>) -> String {
    match ext {
        Some(ext) => format!("{id}.{ext}"),
        None => id.to_string(),
    }
}

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

/// A string-list field: absent or null is `[]`, a list keeps its string members, anything
/// else fails validation.
fn string_list(meta: &Meta, key: &str) -> Option<Vec<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(value) => value.as_str_list(),
    }
}

/// An optional string field: absent or null is `None`, a string is `Some`, anything else
/// fails validation.
fn optional_string(meta: &Meta, key: &str) -> Option<Option<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value.as_str().map(|s| Some(s.to_string())),
    }
}

impl FromMeta for AssetSidecar {
    /// Validate frontmatter as an asset sidecar.
    ///
    /// `id` must carry the `a-` prefix, `type` must be the literal `asset`, and every key
    /// mesh itself always writes — `title`, `created`, `updated`, `filename`, `media_type`,
    /// `bytes`, `sha256`, `blob` — must be present and well typed. Anything else is not an
    /// asset: a listing skips the row silently and a read verb reports it as not found.
    fn from_meta(meta: &Meta) -> Option<AssetSidecar> {
        let id = meta_str(meta, "id")?.to_string();
        if !id.starts_with(ASSET_ID_PREFIX) {
            return None;
        }
        if meta_str(meta, "type") != Some(ASSET_TYPE) {
            return None;
        }
        let bytes = u64::try_from(meta_int(meta, "bytes")?).ok()?;
        Some(AssetSidecar {
            id,
            title: meta_str(meta, "title")?.to_string(),
            tags: string_list(meta, "tags")?,
            owner: optional_string(meta, "owner")?,
            created: Some(meta_time(meta, "created")?),
            updated: Some(meta_time(meta, "updated")?),
            related: string_list(meta, "related")?,
            filename: meta_str(meta, "filename")?.to_string(),
            media_type: meta_str(meta, "media_type")?.to_string(),
            bytes,
            sha256: meta_str(meta, "sha256")?.to_string(),
            blob: meta_str(meta, "blob")?.to_string(),
            meta: meta.clone(),
        })
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::fm::parse_meta;

    const FULL: &str = "id: a-7Q3K\ntype: asset\ntitle: photo.png\ntags:\n  - trip\n\
                        owner: alice\ncreated: 2026-01-02T03:04:05Z\n\
                        updated: 2026-01-03T00:00:00Z\nrelated:\n  - n-1\n\
                        filename: photo.png\nmedia_type: image/png\nbytes: 12\n\
                        sha256: abc\nblob: a-7Q3K.png\n";

    fn meta(yaml: &str) -> Meta {
        parse_meta(yaml).unwrap()
    }

    #[test]
    fn a_full_sidecar_validates() {
        let asset = AssetSidecar::from_meta(&meta(FULL)).unwrap();
        assert_eq!(asset.id, "a-7Q3K");
        assert_eq!(asset.title, "photo.png");
        assert_eq!(asset.tags, ["trip"]);
        assert_eq!(asset.owner.as_deref(), Some("alice"));
        assert_eq!(asset.related, ["n-1"]);
        assert_eq!(asset.filename, "photo.png");
        assert_eq!(asset.media_type, "image/png");
        assert_eq!(asset.bytes, 12);
        assert_eq!(asset.sha256, "abc");
        assert_eq!(asset.blob, "a-7Q3K.png");
    }

    #[test]
    fn every_structural_key_is_required() {
        for key in [
            "id",
            "type",
            "title",
            "created",
            "updated",
            "filename",
            "media_type",
            "bytes",
            "sha256",
            "blob",
        ] {
            let stripped: String = FULL
                .lines()
                .filter(|line| !line.starts_with(&format!("{key}:")))
                .map(|line| format!("{line}\n"))
                .collect();
            assert!(
                AssetSidecar::from_meta(&meta(&stripped)).is_none(),
                "{key} should be required"
            );
        }
    }

    #[test]
    fn a_foreign_id_or_type_is_not_an_asset() {
        let wrong_prefix = FULL.replace("id: a-7Q3K", "id: n-7Q3K");
        assert!(AssetSidecar::from_meta(&meta(&wrong_prefix)).is_none());
        let wrong_type = FULL.replace("type: asset", "type: note");
        assert!(AssetSidecar::from_meta(&meta(&wrong_type)).is_none());
    }

    #[test]
    fn a_negative_or_non_integer_size_fails_validation() {
        let negative = FULL.replace("bytes: 12", "bytes: -1");
        assert!(AssetSidecar::from_meta(&meta(&negative)).is_none());
        let text = FULL.replace("bytes: 12", "bytes: \"12\"");
        assert!(AssetSidecar::from_meta(&meta(&text)).is_none());
    }

    #[test]
    fn unknown_keys_survive_on_the_view() {
        let extended = format!("{FULL}custom_key: keep me\n");
        let asset = AssetSidecar::from_meta(&meta(&extended)).unwrap();
        assert_eq!(meta_str(&asset.meta, "custom_key"), Some("keep me"));
        assert!(!ASSET_FIELDS.is_known("custom_key"));
    }

    #[test]
    fn sort_values_cover_every_key_the_listing_offers() {
        let asset = AssetSidecar::from_meta(&meta(FULL)).unwrap();
        assert_eq!(
            asset.sort_value(SortKey::Title),
            SortValue::Text("photo.png".into())
        );
        assert_eq!(asset.sort_value(SortKey::Bytes), SortValue::Num(12));
        assert_eq!(
            asset.sort_value(SortKey::Created),
            SortValue::Time(asset.created)
        );
        assert_eq!(
            asset.sort_value(SortKey::Updated),
            SortValue::Time(asset.updated)
        );
    }

    #[test]
    fn the_media_table_maps_the_common_extensions() {
        assert_eq!(media_type_for(Some("png")), "image/png");
        assert_eq!(media_type_for(Some("jpg")), "image/jpeg");
        assert_eq!(media_type_for(Some("jpeg")), "image/jpeg");
        assert_eq!(media_type_for(Some("pdf")), "application/pdf");
        assert_eq!(media_type_for(Some("md")), "text/markdown");
        assert_eq!(media_type_for(Some("yaml")), "application/yaml");
        assert_eq!(media_type_for(Some("yml")), "application/yaml");
        assert_eq!(media_type_for(Some("mp4")), "video/mp4");
        assert_eq!(media_type_for(Some("nope")), DEFAULT_MEDIA_TYPE);
        assert_eq!(media_type_for(None), DEFAULT_MEDIA_TYPE);
    }

    #[test]
    fn the_media_table_has_no_duplicate_extensions() {
        let mut seen: Vec<&str> = Vec::new();
        for (ext, _) in MEDIA_TYPES {
            assert!(!seen.contains(&ext), "duplicate extension {ext}");
            seen.push(ext);
        }
        assert_eq!(seen.len(), MEDIA_TYPES.len());
    }

    #[test]
    fn the_extension_rule_lowercases_and_rejects_everything_else() {
        assert_eq!(blob_extension("photo.PNG").as_deref(), Some("png"));
        assert_eq!(blob_extension("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(blob_extension("data.7z").as_deref(), Some("7z"));
        // No dot, a leading dot, an empty extension, punctuation and over-long lose it.
        assert_eq!(blob_extension("README"), None);
        assert_eq!(blob_extension(".hidden"), None);
        assert_eq!(blob_extension("trailing."), None);
        assert_eq!(blob_extension("weird.p!ng"), None);
        assert_eq!(blob_extension("weird.pn g"), None);
        assert_eq!(blob_extension("long.abcdefghijklm"), None);
        assert_eq!(
            blob_extension("long.abcdefghijkl").as_deref(),
            Some("abcdefghijkl")
        );
        // A hostile basename never contributes a path component.
        assert_eq!(blob_extension("../evil.png").as_deref(), Some("png"));
        assert_eq!(blob_extension("evil.png/../x"), None);
    }

    #[test]
    fn the_blob_name_is_the_id_plus_the_kept_extension() {
        assert_eq!(blob_name("a-7Q3K", Some("png")), "a-7Q3K.png");
        assert_eq!(blob_name("a-7Q3K", None), "a-7Q3K");
    }
}
