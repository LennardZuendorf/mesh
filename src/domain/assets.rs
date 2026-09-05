//! Asset ingest, sidecars, attach/detach and gc.
//!
//! An asset is two files that share a stem: the blob `<assets>/<id>.<ext>` and the sidecar
//! `<assets>/<id>.md`. The id *is* the content address, so ingest is idempotent by content
//! and the universal "a mesh file is named `<id>.md`" invariant still holds.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::select::{select, FromMeta};
use crate::domain::{effective_owner, memories, notes, resolve_wikilinks, tasks, validate_owner};
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::{read_doc, read_meta_only, write_doc, Doc, Meta, Row, Value, View};
use crate::ids::{content_id, sha256_hex};
use crate::model::asset::{
    blob_extension, blob_name, media_type_for, AssetSidecar, AssetSummary, GcReport, ASSET_TYPE,
};
use crate::model::common::{meta_str, meta_strings, optional_str, ts_value};
use crate::spaces::Space;
use crate::storage::lock::{create_lock, entity_lock, hold};
use crate::storage::{iter_md, safe_resolve};
use crate::text::edit_distance;
use crate::timefmt::now_utc;

/// How many near-miss ids a not-found error carries.
const MAX_CANDIDATES: usize = 5;

/// The spaces whose `related` lists can name an asset.
const REFERRING_SPACES: [Space; 4] = [Space::Notes, Space::Tasks, Space::Memories, Space::Assets];

/// What `asset add` was asked to store.
#[derive(Clone, Debug, Default)]
pub struct NewAsset {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub caption: String,
    pub attach: Option<String>,
}

/// The outcome of an ingest, including the content-address dedupe branch.
#[derive(Clone, Debug)]
pub struct AddOutcome {
    pub asset: AssetSidecar,
    pub deduplicated: bool,
}

// ---------------------------------------------------------------------------------------
// paths and resolution
// ---------------------------------------------------------------------------------------

fn root(cfg: &Config) -> Result<&Path> {
    cfg.root(Space::Assets)
}

fn stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

/// Every sidecar path in the assets space, sorted.
fn sidecar_paths(cfg: &Config) -> Vec<PathBuf> {
    let Ok(root) = root(cfg) else {
        return Vec::new();
    };
    iter_md(root, true, cfg.spaces.exclusions_for(Space::Assets)).collect()
}

fn asset_not_found(id: &str) -> MeshError {
    MeshError::AssetNotFound(id.to_string())
}

/// The five ids closest to `target` by edit distance.
fn candidates(cfg: &Config, target: &str) -> Vec<String> {
    let lower = target.to_lowercase();
    let mut scored: Vec<(usize, String)> = sidecar_paths(cfg)
        .iter()
        .filter_map(|p| stem(p))
        .map(|id| (edit_distance(&lower, &id.to_lowercase()), id.to_string()))
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, id)| id)
        .collect()
}

/// Resolve an asset id to its sidecar path.
///
/// Assets are addressed by id only: a content address is not a slug, and asset titles are
/// filenames that collide constantly.
pub fn resolve(cfg: &Config, id: &str) -> Result<PathBuf> {
    root(cfg)?;
    for path in sidecar_paths(cfg) {
        if stem(&path) == Some(id) {
            return safe_resolve(&cfg.spaces, &path);
        }
    }
    Err(asset_not_found(id).with_candidates(candidates(cfg, id)))
}

// ---------------------------------------------------------------------------------------
// reads
// ---------------------------------------------------------------------------------------

/// Every readable `(path, frontmatter)` pair in the assets space.
pub fn rows(cfg: &Config) -> Vec<Row> {
    sidecar_paths(cfg)
        .into_iter()
        .filter_map(|path| read_meta_only(&path).map(|meta| Row { path, meta }))
        .collect()
}

/// Read one sidecar: frontmatter, caption and path.
pub fn get(cfg: &Config, id: &str) -> Result<View<AssetSidecar>> {
    let path = resolve(cfg, id)?;
    let Some(doc) = read_doc(&path) else {
        return Err(asset_not_found(id));
    };
    let item = AssetSidecar::from_meta(&doc.meta).ok_or_else(|| asset_not_found(id))?;
    Ok(View {
        item,
        body: doc.body,
        path,
    })
}

/// The absolute path of an asset's blob. Exit 3 when either the sidecar or the blob is gone.
pub fn blob_path(cfg: &Config, id: &str) -> Result<PathBuf> {
    let view = get(cfg, id)?;
    let root = root(cfg)?.to_path_buf();
    let path = safe_resolve(&cfg.spaces, &root.join(&view.item.blob))?;
    if !path.is_file() {
        return Err(asset_not_found(id));
    }
    Ok(path)
}

/// List assets. `media` is an exact match against the sidecar's `media_type`.
pub fn list(cfg: &Config, f: &Filter, media: Option<&str>) -> Result<Vec<View<AssetSidecar>>> {
    root(cfg)?;
    let filter = f.clone().with_extra("media_type", media);
    Ok(select(rows(cfg), &filter))
}

// ---------------------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------------------

/// The source's basename — kept as data, never as a path component.
fn source_basename(src: &Path) -> String {
    src.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| src.to_string_lossy().into_owned())
}

/// Write bytes atomically: sibling temp → mode → fsync → rename.
///
/// `storage::atomic_write` takes UTF-8 text; a blob is bytes, so this is its binary twin.
fn write_blob(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("blob");
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    set_fresh_mode(temp.as_file());
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| MeshError::Io(e.error))?;
    if let Ok(handle) = std::fs::File::open(parent) {
        let _ = handle.sync_all();
    }
    Ok(())
}

fn set_fresh_mode(file: &std::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    let mode = crate::storage::atomic::fresh_file_mode();
    let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
}

/// The stored asset whose bytes hash to `digest`, if any.
fn find_by_digest(cfg: &Config, digest: &str) -> Option<AssetSidecar> {
    rows(cfg).into_iter().find_map(|row| {
        let asset = AssetSidecar::from_meta(&row.meta)?;
        (asset.sha256 == digest).then_some(asset)
    })
}

/// Copy a file into the assets space, content-addressed, with its sidecar.
///
/// Idempotent by content: identical bytes return the stored asset, write nothing and leave
/// `updated` alone. The blob is written **before** the sidecar — a crash between them leaves
/// an orphan blob `asset gc` finds, where the reverse would leave every read verb, search hit
/// and graph node pointing at nothing. A failed sidecar write unlinks the blob.
pub fn add(cfg: &Config, src: &Path, o: NewAsset) -> Result<AddOutcome> {
    let root = root(cfg)?.to_path_buf();
    let bytes = std::fs::read(src)
        .map_err(|e| MeshError::Validation(format!("cannot read {}: {e}", src.display())))?;
    validate_owner(cfg, o.owner.as_deref())?;

    let filename = source_basename(src);
    let title = o
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| filename.clone());
    let digest = sha256_hex(&bytes);

    let outcome = {
        // Id allocation, the dedupe check and both writes share the create lock.
        let _guard = hold(&create_lock(&root))?;
        match find_by_digest(cfg, &digest) {
            Some(asset) => AddOutcome {
                asset,
                deduplicated: true,
            },
            None => AddOutcome {
                asset: store(cfg, &root, &bytes, &digest, &filename, &title, &o)?,
                deduplicated: false,
            },
        }
    };

    match &o.attach {
        Some(target) => Ok(AddOutcome {
            asset: attach(cfg, &outcome.asset.id, target, None)?,
            deduplicated: outcome.deduplicated,
        }),
        None => Ok(outcome),
    }
}

/// The ingest half of [`add`], run under the create lock.
#[allow(clippy::too_many_arguments)]
fn store(
    cfg: &Config,
    root: &Path,
    bytes: &[u8],
    digest: &str,
    filename: &str,
    title: &str,
    o: &NewAsset,
) -> Result<AssetSidecar> {
    let taken: Vec<String> = sidecar_paths(cfg)
        .iter()
        .filter_map(|p| stem(p))
        .map(str::to_string)
        .collect();
    let id = content_id(bytes, &|candidate| taken.iter().any(|t| t == candidate));
    let ext = blob_extension(filename);
    let blob = blob_name(&id, ext.as_deref());
    let blob_dest = safe_resolve(&cfg.spaces, &root.join(&blob))?;
    let sidecar = safe_resolve(&cfg.spaces, &root.join(format!("{id}.md")))?;

    let now = now_utc();
    let mut meta = Meta::new();
    meta.insert("id".to_string(), Value::str(id.as_str()));
    meta.insert("type".to_string(), Value::str(ASSET_TYPE));
    meta.insert("title".to_string(), Value::str(title));
    meta.insert("tags".to_string(), Value::strings(o.tags.clone()));
    meta.insert(
        "owner".to_string(),
        optional_str(effective_owner(cfg, o.owner.as_deref()).as_deref()),
    );
    meta.insert("created".to_string(), ts_value(&now));
    meta.insert("updated".to_string(), ts_value(&now));
    meta.insert(
        "related".to_string(),
        Value::strings(resolve_wikilinks(cfg, &o.caption)),
    );
    meta.insert("filename".to_string(), Value::str(filename));
    meta.insert(
        "media_type".to_string(),
        Value::str(media_type_for(ext.as_deref())),
    );
    meta.insert(
        "bytes".to_string(),
        Value::Int(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
    );
    meta.insert("sha256".to_string(), Value::str(digest));
    meta.insert("blob".to_string(), Value::str(blob.as_str()));

    write_blob(&blob_dest, bytes)?;
    let doc = Doc::new(meta, o.caption.clone());
    if let Err(e) = write_doc(&cfg.spaces, &sidecar, &doc) {
        // A sidecar nobody can read is worse than a blob gc can sweep.
        let _ = std::fs::remove_file(&blob_dest);
        return Err(e);
    }
    AssetSidecar::from_meta(&doc.meta).ok_or_else(|| asset_not_found(&id))
}

// ---------------------------------------------------------------------------------------
// attach / detach
// ---------------------------------------------------------------------------------------

/// The space a `n-`/`t-`/`m-` target id addresses.
fn target_space(target: &str) -> Result<Space> {
    for space in [Space::Notes, Space::Tasks, Space::Memories] {
        if space
            .id_prefix()
            .is_some_and(|prefix| target.starts_with(prefix))
        {
            return Ok(space);
        }
    }
    Err(MeshError::Validation(format!(
        "invalid target id: '{target}' (use an n-, t- or m- id)"
    )))
}

/// The file a target id names, resolved in its own space.
fn target_path(cfg: &Config, space: Space, target: &str) -> Result<PathBuf> {
    match space {
        Space::Notes => notes::resolve(cfg, target),
        Space::Tasks => tasks::resolve(cfg, target),
        _ => {
            let root = cfg.root(space)?;
            for path in iter_md(root, true, cfg.spaces.exclusions_for(space)) {
                if stem(&path) == Some(target) {
                    return safe_resolve(&cfg.spaces, &path);
                }
            }
            Err(MeshError::MemoryNotFound(target.to_string()))
        }
    }
}

/// Append the embed block to a target's body through that space's ordinary append verb.
fn append_embed(
    cfg: &Config,
    space: Space,
    target: &str,
    block: &str,
    o: AppendOpts,
) -> Result<()> {
    match space {
        Space::Notes => notes::append(cfg, target, block, o).map(|_| ()),
        Space::Tasks => tasks::append(cfg, target, block, o).map(|_| ()),
        _ => memories::append(cfg, target, block, o).map(|_| ()),
    }
}

/// Add or remove one id in a `related` list, in place. Returns whether anything changed;
/// a change also bumps `updated`.
fn edit_related(meta: &mut Meta, value: &str, add: bool) -> bool {
    let mut related = meta_strings(meta, "related");
    let changed = if add {
        if related.iter().any(|r| r == value) {
            false
        } else {
            related.push(value.to_string());
            true
        }
    } else {
        let before = related.len();
        related.retain(|r| r != value);
        before != related.len()
    };
    if changed {
        meta.insert("related".to_string(), Value::strings(related));
        meta.insert("updated".to_string(), ts_value(&now_utc()));
    }
    changed
}

/// Amend a target entity's `related` under its own entity lock. A no-op never rewrites.
fn amend_target_related(
    cfg: &Config,
    space: Space,
    target: &str,
    value: &str,
    add: bool,
) -> Result<()> {
    let root = cfg.root(space)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, target))?;
    // Re-resolved inside the lock: a task can move between open/ and done/ underneath us.
    let path = target_path(cfg, space, target)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(asset_not_found(target));
    };
    if edit_related(&mut doc.meta, value, add) {
        write_doc(&cfg.spaces, &path, &doc)?;
    }
    Ok(())
}

/// Amend the sidecar's `related` under the asset's entity lock. A no-op never rewrites.
fn amend_sidecar_related(cfg: &Config, id: &str, value: &str, add: bool) -> Result<AssetSidecar> {
    let root = root(cfg)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, id))?;
    let path = resolve(cfg, id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(asset_not_found(id));
    };
    let changed = edit_related(&mut doc.meta, value, add);
    let asset = AssetSidecar::from_meta(&doc.meta).ok_or_else(|| asset_not_found(id))?;
    if changed {
        write_doc(&cfg.spaces, &path, &doc)?;
    }
    Ok(asset)
}

/// Attach an asset to a note, task or memory.
///
/// The embed `![[<blob>]]` goes into the target's body through the ordinary append path, then
/// both `related` lists learn about each other — the target's, so `graph --direction out`
/// finds the asset, and the sidecar's, so `--direction in` finds the referrer.
pub fn attach(cfg: &Config, id: &str, target: &str, section: Option<&str>) -> Result<AssetSidecar> {
    let view = get(cfg, id)?;
    let space = target_space(target)?;
    let path = target_path(cfg, space, target)?;
    let block = format!("![[{}]]", view.item.blob);

    // Attaching twice must not duplicate the embed: the body is the agent's, not ours.
    let body = read_doc(&path).map(|d| d.body).unwrap_or_default();
    if !body.contains(&block) {
        append_embed(
            cfg,
            space,
            target,
            &block,
            AppendOpts {
                section: section.map(str::to_string),
                ..AppendOpts::default()
            },
        )?;
    }
    amend_target_related(cfg, space, target, id, true)?;
    amend_sidecar_related(cfg, id, target, true)
}

/// Detach an asset from an entity: both `related` lists drop the link, and the body is left
/// exactly as the agent wrote it.
pub fn detach(cfg: &Config, id: &str, target: &str) -> Result<AssetSidecar> {
    get(cfg, id)?;
    let space = target_space(target)?;
    target_path(cfg, space, target)?;
    amend_target_related(cfg, space, target, id, false)?;
    amend_sidecar_related(cfg, id, target, false)
}

// ---------------------------------------------------------------------------------------
// remove / gc
// ---------------------------------------------------------------------------------------

/// Every entity id whose `related` names this asset.
pub fn references(cfg: &Config, id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for space in REFERRING_SPACES {
        let Ok(root) = cfg.root(space) else {
            continue;
        };
        for path in iter_md(root, true, cfg.spaces.exclusions_for(space)) {
            let Some(meta) = read_meta_only(&path) else {
                continue;
            };
            let Some(other) = meta_str(&meta, "id") else {
                continue;
            };
            if other == id {
                continue;
            }
            if meta_strings(&meta, "related").iter().any(|r| r == id) {
                out.push(other.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The delete refusal, so the CLI can raise it *before* it prompts and `remove` can raise it
/// again inside its own lock.
pub fn check_removable(cfg: &Config, id: &str) -> Result<()> {
    let refs = references(cfg, id);
    if refs.is_empty() {
        return Ok(());
    }
    Err(MeshError::Validation(format!(
        "asset {id} is referenced by {} entities; pass --force",
        refs.len()
    )))
}

/// Delete a sidecar and its blob. Without `force`, a referenced asset is refused (exit 2).
pub fn remove(cfg: &Config, id: &str, force: bool) -> Result<String> {
    let path = resolve(cfg, id)?;
    let asset_id = stem(&path)
        .map(str::to_string)
        .ok_or_else(|| asset_not_found(id))?;
    if !force {
        check_removable(cfg, &asset_id)?;
    }
    let root = root(cfg)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, &asset_id))?;
    let path = resolve(cfg, &asset_id)?;
    let blobs = blob_names_for(&root, &path, &asset_id);
    // Sidecar first: an unreferenced blob is sweepable, a sidecar pointing at nothing is not.
    std::fs::remove_file(&path)?;
    for name in blobs {
        if let Ok(blob) = safe_resolve(&cfg.spaces, &root.join(name)) {
            let _ = std::fs::remove_file(blob);
        }
    }
    Ok(asset_id)
}

/// The blob a sidecar points at, or — when the sidecar is corrupt — every sibling file that
/// shares its stem. Removing a corrupt asset is the repair path, so it takes both.
fn blob_names_for(root: &Path, sidecar: &Path, id: &str) -> Vec<String> {
    if let Some(name) =
        read_meta_only(sidecar).and_then(|m| meta_str(&m, "blob").map(str::to_string))
    {
        return vec![name];
    }
    blob_entries(root)
        .into_iter()
        .filter(|name| blob_stem(name) == id)
        .collect()
}

/// Every non-Markdown file directly in the assets root, sorted: the blob candidates.
fn blob_entries(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".md") {
            continue;
        }
        out.push(name);
    }
    out.sort();
    out
}

/// A blob filename's stem: everything before the last dot.
fn blob_stem(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// Report blobs with no sidecar and sidecars with no blob. `apply` deletes orphan blobs only
/// — a sidecar is metadata the operator may still want, and mesh never guesses at that.
pub fn gc(cfg: &Config, apply: bool) -> Result<GcReport> {
    let root = root(cfg)?.to_path_buf();
    let stems: Vec<String> = sidecar_paths(cfg)
        .iter()
        .filter_map(|p| stem(p))
        .map(str::to_string)
        .collect();

    let orphan_blobs: Vec<String> = blob_entries(&root)
        .into_iter()
        .filter(|name| !stems.iter().any(|s| s == blob_stem(name)))
        .collect();

    let mut orphan_sidecars: Vec<String> = Vec::new();
    for path in sidecar_paths(cfg) {
        let Some(id) = stem(&path).map(str::to_string) else {
            continue;
        };
        let blob = read_meta_only(&path).and_then(|m| meta_str(&m, "blob").map(str::to_string));
        let present = blob
            .as_ref()
            .and_then(|name| safe_resolve(&cfg.spaces, &root.join(name)).ok())
            .is_some_and(|p| p.is_file());
        if !present {
            orphan_sidecars.push(id);
        }
    }
    orphan_sidecars.sort();

    let mut removed = 0u64;
    if apply {
        for name in &orphan_blobs {
            let Ok(path) = safe_resolve(&cfg.spaces, &root.join(name)) else {
                continue;
            };
            if std::fs::remove_file(path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(GcReport {
        orphan_blobs,
        orphan_sidecars,
        removed,
    })
}

/// The `status` payload's assets block: valid sidecars, their total size, orphan blobs.
pub fn summary(cfg: &Config) -> AssetSummary {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for row in rows(cfg) {
        if let Some(asset) = AssetSidecar::from_meta(&row.meta) {
            count += 1;
            bytes = bytes.saturating_add(asset.bytes);
        }
    }
    let orphan_blobs = gc(cfg, false)
        .map(|r| u64::try_from(r.orphan_blobs.len()).unwrap_or(0))
        .unwrap_or(0);
    AssetSummary {
        count,
        bytes,
        orphan_blobs,
    }
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
    use crate::config::test_support::config_for;
    use crate::domain::notes::NewNote;
    use crate::domain::SortKey;

    struct Vault {
        dir: tempfile::TempDir,
        cfg: Config,
    }

    fn vault() -> Vault {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let cfg = config_for(&root);
        Vault { dir, cfg }
    }

    /// A source file outside the vault, so ingest always copies across a boundary.
    fn source(v: &Vault, name: &str, bytes: &[u8]) -> PathBuf {
        let path = v.dir.path().join("src").join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn add_file(v: &Vault, name: &str, bytes: &[u8]) -> AssetSidecar {
        let src = source(v, name, bytes);
        add(&v.cfg, &src, NewAsset::default()).unwrap().asset
    }

    fn assets_root(v: &Vault) -> PathBuf {
        v.cfg.root(Space::Assets).unwrap().to_path_buf()
    }

    #[test]
    fn add_writes_a_blob_and_a_sidecar_that_share_a_stem() {
        let v = vault();
        let asset = add_file(&v, "photo.png", b"\x89PNG\r\n\x1a\n binary");
        assert!(asset.id.starts_with("a-"));
        assert_eq!(asset.blob, format!("{}.png", asset.id));
        assert_eq!(asset.media_type, "image/png");
        assert_eq!(asset.filename, "photo.png");
        assert_eq!(asset.title, "photo.png");
        assert_eq!(asset.bytes, 15);
        assert_eq!(asset.sha256, sha256_hex(b"\x89PNG\r\n\x1a\n binary"));
        let root = assets_root(&v);
        assert!(root.join(&asset.blob).is_file());
        assert!(root.join(format!("{}.md", asset.id)).is_file());
        assert_eq!(
            std::fs::read(root.join(&asset.blob)).unwrap(),
            b"\x89PNG\r\n\x1a\n binary"
        );
    }

    #[test]
    fn the_id_is_the_content_address() {
        let v = vault();
        let first = add_file(&v, "a.txt", b"same bytes");
        let other = vault();
        let second = add_file(&other, "b.bin", b"same bytes");
        assert_eq!(first.id, second.id);
        assert_eq!(first.sha256, second.sha256);
        // The extension and the filename follow the source; the id never does.
        assert_eq!(first.blob, format!("{}.txt", first.id));
        assert_eq!(second.blob, format!("{}.bin", second.id));
    }

    #[test]
    fn identical_bytes_dedupe_without_touching_the_sidecar() {
        let v = vault();
        let first = add_file(&v, "a.txt", b"hello");
        let sidecar = assets_root(&v).join(format!("{}.md", first.id));
        let before = std::fs::read(&sidecar).unwrap();
        let src = source(&v, "copy.txt", b"hello");
        let outcome = add(&v.cfg, &src, NewAsset::default()).unwrap();
        assert!(outcome.deduplicated);
        assert_eq!(outcome.asset.id, first.id);
        assert_eq!(std::fs::read(&sidecar).unwrap(), before);
        assert_eq!(rows(&v.cfg).len(), 1);
    }

    #[test]
    fn the_extension_rule_never_lets_a_filename_traverse() {
        let v = vault();
        let hostile = add_file(&v, "..\\..\\evil.png", b"x");
        assert_eq!(hostile.filename, "..\\..\\evil.png");
        assert_eq!(hostile.blob, format!("{}.png", hostile.id));
        assert!(assets_root(&v).join(&hostile.blob).is_file());
        assert!(!v.cfg.vault().join("evil.png").exists());
    }

    #[test]
    fn an_unusable_extension_is_dropped_and_the_media_type_defaults() {
        let v = vault();
        let plain = add_file(&v, "README", b"text");
        assert_eq!(plain.blob, plain.id);
        assert_eq!(plain.media_type, "application/octet-stream");
        let long = add_file(&v, "x.abcdefghijklm", b"other");
        assert_eq!(long.blob, long.id);
    }

    #[test]
    fn an_unreadable_source_is_a_validation_error_and_writes_nothing() {
        let v = vault();
        let missing = v.cfg.vault().join("nope.png");
        let err = add(&v.cfg, &missing, NewAsset::default()).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(
            err.to_string()
                .starts_with(&format!("cannot read {}: ", missing.display())),
            "{err}"
        );
        assert!(rows(&v.cfg).is_empty());
    }

    #[test]
    fn the_owner_roster_is_enforced_before_any_write() {
        let mut v = vault();
        v.cfg.tasks.collections = vec!["alice".into()];
        let src = source(&v, "a.txt", b"x");
        let err = add(
            &v.cfg,
            &src,
            NewAsset {
                owner: Some("ghost".into()),
                ..NewAsset::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "unknown owner: 'ghost'");
        assert!(rows(&v.cfg).is_empty());
    }

    #[test]
    fn a_caption_becomes_the_body_and_its_wikilinks_become_related() {
        let v = vault();
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let src = source(&v, "a.txt", b"x");
        let asset = add(
            &v.cfg,
            &src,
            NewAsset {
                caption: format!("see [[{}]]", note.id),
                ..NewAsset::default()
            },
        )
        .unwrap()
        .asset;
        assert_eq!(asset.related, [note.id.as_str()]);
        assert_eq!(
            get(&v.cfg, &asset.id).unwrap().body,
            format!("see [[{}]]", note.id)
        );
    }

    #[test]
    fn attach_embeds_the_blob_and_links_both_related_lists() {
        let v = vault();
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let asset = add_file(&v, "photo.png", b"bytes");
        let after = attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        assert_eq!(after.related, [note.id.as_str()]);
        let view = notes::get(&v.cfg, &note.id).unwrap();
        assert!(
            view.body.contains(&format!("![[{}]]", asset.blob)),
            "{}",
            view.body
        );
        assert_eq!(view.item.related, [asset.id.as_str()]);
    }

    #[test]
    fn attaching_twice_is_a_no_op_on_both_files() {
        let v = vault();
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let asset = add_file(&v, "photo.png", b"bytes");
        attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        let note_path = notes::resolve(&v.cfg, &note.id).unwrap();
        let sidecar = resolve(&v.cfg, &asset.id).unwrap();
        let before = (
            std::fs::read(&note_path).unwrap(),
            std::fs::read(&sidecar).unwrap(),
        );
        attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        assert_eq!(std::fs::read(&note_path).unwrap(), before.0);
        assert_eq!(std::fs::read(&sidecar).unwrap(), before.1);
    }

    #[test]
    fn detach_clears_both_related_lists_and_keeps_the_body() {
        let v = vault();
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let asset = add_file(&v, "photo.png", b"bytes");
        attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        let embed = format!("![[{}]]", asset.blob);
        let after = detach(&v.cfg, &asset.id, &note.id).unwrap();
        assert!(after.related.is_empty());
        let view = notes::get(&v.cfg, &note.id).unwrap();
        assert!(view.item.related.is_empty());
        assert!(view.body.contains(&embed), "the body belongs to the agent");
        // A second detach changes nothing.
        let sidecar = resolve(&v.cfg, &asset.id).unwrap();
        let before = std::fs::read(&sidecar).unwrap();
        detach(&v.cfg, &asset.id, &note.id).unwrap();
        assert_eq!(std::fs::read(&sidecar).unwrap(), before);
    }

    #[test]
    fn a_bad_target_prefix_is_a_validation_error() {
        let v = vault();
        let asset = add_file(&v, "a.txt", b"x");
        let err = attach(&v.cfg, &asset.id, "x-1234", None).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "invalid target id: 'x-1234' (use an n-, t- or m- id)"
        );
        assert_eq!(
            attach(&v.cfg, &asset.id, "n-9999", None)
                .unwrap_err()
                .code(),
            3
        );
    }

    #[test]
    fn remove_refuses_a_referenced_asset_without_force() {
        let v = vault();
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let asset = add_file(&v, "photo.png", b"bytes");
        attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        let err = remove(&v.cfg, &asset.id, false).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            format!(
                "asset {} is referenced by 1 entities; pass --force",
                asset.id
            )
        );
        assert!(resolve(&v.cfg, &asset.id).is_ok());
        assert_eq!(remove(&v.cfg, &asset.id, true).unwrap(), asset.id);
        let root = assets_root(&v);
        assert!(!root.join(format!("{}.md", asset.id)).exists());
        assert!(!root.join(&asset.blob).exists());
    }

    #[test]
    fn references_spans_every_space_and_ignores_the_asset_itself() {
        let v = vault();
        let asset = add_file(&v, "photo.png", b"bytes");
        let note = notes::create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        attach(&v.cfg, &asset.id, &note.id, None).unwrap();
        let memories_root = v.cfg.vault().join("memories");
        std::fs::create_dir_all(&memories_root).unwrap();
        std::fs::write(
            memories_root.join("m-AAAA.md"),
            format!(
                "---\nid: m-AAAA\ntype: memory\ntitle: M\nrelated:\n  - {}\n---\n\nx\n",
                asset.id
            ),
        )
        .unwrap();
        let mut expected = vec![note.id.clone(), "m-AAAA".to_string()];
        expected.sort();
        assert_eq!(references(&v.cfg, &asset.id), expected);
    }

    #[test]
    fn a_corrupt_sidecar_is_not_found_but_still_removable() {
        let v = vault();
        let root = assets_root(&v);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a-BAD.md"),
            "---\nid: a-BAD\ntitle: [oops\n---\n\nx\n",
        )
        .unwrap();
        std::fs::write(root.join("a-BAD.png"), b"blob").unwrap();
        assert_eq!(get(&v.cfg, "a-BAD").unwrap_err().code(), 3);
        assert_eq!(blob_path(&v.cfg, "a-BAD").unwrap_err().code(), 3);
        assert_eq!(remove(&v.cfg, "a-BAD", false).unwrap(), "a-BAD");
        assert!(!root.join("a-BAD.md").exists());
        assert!(!root.join("a-BAD.png").exists(), "the blob goes too");
    }

    #[test]
    fn gc_reports_orphans_and_only_apply_removes_blobs() {
        let v = vault();
        let asset = add_file(&v, "photo.png", b"bytes");
        let root = assets_root(&v);
        std::fs::write(root.join("stray.bin"), b"junk").unwrap();
        std::fs::write(
            root.join("a-GHOST.md"),
            "---\nid: a-GHOST\ntype: asset\ntitle: g\ntags: []\nowner: null\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
             filename: g.png\nmedia_type: image/png\nbytes: 1\nsha256: ff\n\
             blob: a-GHOST.png\n---\n\nx\n",
        )
        .unwrap();

        let report = gc(&v.cfg, false).unwrap();
        assert_eq!(report.orphan_blobs, ["stray.bin"]);
        assert_eq!(report.orphan_sidecars, ["a-GHOST"]);
        assert_eq!(report.removed, 0);
        assert!(root.join("stray.bin").is_file());

        let applied = gc(&v.cfg, true).unwrap();
        assert_eq!(applied.removed, 1);
        assert!(!root.join("stray.bin").exists());
        assert!(
            root.join("a-GHOST.md").is_file(),
            "sidecars are never swept"
        );
        assert!(root.join(&asset.blob).is_file());
    }

    #[test]
    fn summary_counts_valid_sidecars_and_orphan_blobs() {
        let v = vault();
        add_file(&v, "a.txt", b"12345");
        add_file(&v, "b.txt", b"123");
        std::fs::write(assets_root(&v).join("stray.bin"), b"junk").unwrap();
        let s = summary(&v.cfg);
        assert_eq!(s.count, 2);
        assert_eq!(s.bytes, 8);
        assert_eq!(s.orphan_blobs, 1);
    }

    #[test]
    fn list_filters_by_media_type_and_sorts_by_size() {
        let v = vault();
        let small = add_file(&v, "s.png", b"1");
        let big = add_file(&v, "b.png", b"1234567");
        add_file(&v, "t.txt", b"text");
        let by_bytes = list(
            &v.cfg,
            &Filter {
                sort: SortKey::Bytes,
                ..Filter::unbounded()
            },
            Some("image/png"),
        )
        .unwrap();
        let ids: Vec<&str> = by_bytes.iter().map(|v| v.item.id.as_str()).collect();
        assert_eq!(ids, [big.id.as_str(), small.id.as_str()]);
        assert_eq!(list(&v.cfg, &Filter::unbounded(), None).unwrap().len(), 3);
    }

    #[test]
    fn the_blob_path_is_absolute_and_exit_three_when_the_blob_is_gone() {
        let v = vault();
        let asset = add_file(&v, "photo.png", b"bytes");
        let path = blob_path(&v.cfg, &asset.id).unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with(&asset.blob));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(blob_path(&v.cfg, &asset.id).unwrap_err().code(), 3);
        assert_eq!(blob_path(&v.cfg, "a-NOPE").unwrap_err().code(), 3);
    }

    #[test]
    fn a_disabled_assets_space_is_a_validation_error_on_every_verb() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Assets, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        let expected = "space 'assets' is disabled in [spaces]";
        for err in [
            add(&cfg, Path::new("/etc/hostname"), NewAsset::default()).unwrap_err(),
            get(&cfg, "a-1").unwrap_err(),
            blob_path(&cfg, "a-1").unwrap_err(),
            list(&cfg, &Filter::unbounded(), None).unwrap_err(),
            gc(&cfg, false).unwrap_err(),
            remove(&cfg, "a-1", true).unwrap_err(),
        ] {
            assert_eq!(err.code(), 2);
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn a_miss_carries_candidates() {
        let v = vault();
        let asset = add_file(&v, "a.txt", b"x");
        let err = get(&v.cfg, "a-ZZZZZZ").unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "asset not found: a-ZZZZZZ");
        assert_eq!(err.candidates(), [asset.id.as_str()]);
    }
}
