//! Note verbs: create, append, update, get, list, delete, plus the resolution seam every
//! other space's lens calls.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::fm::{
    parse_meta, read_body, read_doc, read_meta_only, split_frontmatter, write_doc, Doc, Meta, Row,
    Value, View,
};
use crate::ids::generate_id;
use crate::model::common::{meta_str, meta_strings, meta_time, optional_str, ts_value};
use crate::model::note::{ForeignView, Note, NOTE_ID_PREFIX, NOTE_TYPES};
use crate::model::{ordered, NOTE_FIELDS};
use crate::spaces::Space;
use crate::storage::lock::{create_lock, entity_lock, hold};
use crate::storage::{iter_md, safe_resolve};
use crate::text::{append_to_end, append_under_section, edit_distance, format_block, slugify};
use crate::timefmt::{iso_z, now_utc};

use crate::domain::select::{matches_filters, select, FromMeta};
pub use crate::domain::AppendOpts;
use crate::domain::{apply_tag_spec, effective_owner, resolve_wikilinks, validate_owner, Filter};

/// How many near-miss ids a not-found error carries.
const MAX_CANDIDATES: usize = 5;

/// What `note new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewNote {
    pub note_type: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
}

/// What `note update` was asked to change.
#[derive(Clone, Debug, Default)]
pub struct UpdateNote {
    pub tags: Option<String>,
    pub new_type: Option<String>,
    pub title: Option<String>,
    pub owner: Option<String>,
}

// ---------------------------------------------------------------------------------------
// paths and resolution
// ---------------------------------------------------------------------------------------

fn stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

fn is_mesh_stem(path: &Path) -> bool {
    stem(path).is_some_and(|s| s.starts_with(NOTE_ID_PREFIX))
}

/// The mesh id a file's frontmatter carries, if any — the one mesh-native test for notes.
fn meta_id(path: &Path) -> Option<String> {
    read_meta_only(path)
        .as_ref()
        .and_then(|m| meta_str(m, "id"))
        .filter(|id| id.starts_with(NOTE_ID_PREFIX))
        .map(str::to_string)
}

/// Every Markdown file in the notes space, sorted, dot components and exclusions skipped.
fn all_paths(cfg: &Config) -> Vec<PathBuf> {
    let Ok(root) = cfg.root(Space::Notes) else {
        return Vec::new();
    };
    iter_md(root, true, cfg.spaces.exclusions_for(Space::Notes)).collect()
}

/// The files a mesh verb may address: the frontmatter carries a mesh id, or the stem is one
/// mesh would mint. Stem membership is kept for the repair path — a `n-`-stemmed file with
/// unparseable frontmatter still resolves, and is then reported not-found, because delete
/// is the only way out of a corrupt file.
fn mesh_paths(cfg: &Config) -> Vec<PathBuf> {
    all_paths(cfg)
        .into_iter()
        .filter(|p| is_mesh_stem(p) || meta_id(p).is_some())
        .collect()
}

/// Foreign Markdown in the notes space: no mesh id in the frontmatter and a stem mesh would
/// never mint. Classification is by id, so an adopted file keeps its foreign stem and still
/// leaves this set — and an `n-`-stemmed file with no id key lands here, honestly.
fn foreign_paths(cfg: &Config) -> Vec<PathBuf> {
    all_paths(cfg)
        .into_iter()
        .filter(|p| !is_mesh_stem(p) && meta_id(p).is_none())
        .collect()
}

/// Up to five nearest ids, by edit distance over both the id and the title slug.
fn candidates(paths: &[PathBuf], target: &str) -> Vec<String> {
    let want = slugify(target);
    let lower = target.to_lowercase();
    let mut scored: Vec<(usize, String)> = Vec::new();
    for path in paths {
        let Some(id) = meta_id(path).or_else(|| stem(path).map(str::to_string)) else {
            continue;
        };
        let mut best = edit_distance(&lower, &id.to_lowercase());
        if let Some(title) = read_meta_only(path)
            .as_ref()
            .and_then(|m| meta_str(m, "title").map(str::to_string))
        {
            best = best.min(edit_distance(&want, &slugify(&title)));
        }
        scored.push((best, id));
    }
    scored.sort();
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, id)| id)
        .collect()
}

/// Resolve an `n-` id or a title slug to the note's path.
///
/// An exact stem match wins (the corrupt-frontmatter repair path); then the frontmatter id
/// — an adopted file keeps its foreign stem, so the id lives in the file, not the name;
/// then the slugified title against every addressable note. Several title matches are an
/// ambiguous slug (exit 2, ids sorted); none is not-found (exit 3) carrying the near-miss
/// candidates.
pub fn resolve(cfg: &Config, target: &str) -> Result<PathBuf> {
    // A disabled space is a validation error on every verb, not an empty corpus: `all_paths`
    // degrades to `[]` so the cross-space scans keep working, so the check belongs here.
    cfg.root(Space::Notes)?;
    let paths = mesh_paths(cfg);
    if let Some(hit) = paths.iter().find(|p| stem(p) == Some(target)) {
        return safe_resolve(&cfg.spaces, hit);
    }
    if let Some(hit) = paths.iter().find(|p| meta_id(p).as_deref() == Some(target)) {
        return safe_resolve(&cfg.spaces, hit);
    }
    let want = slugify(target);
    let mut hits: Vec<&PathBuf> = Vec::new();
    for path in &paths {
        let Some(meta) = read_meta_only(path) else {
            continue;
        };
        if meta_str(&meta, "title").is_some_and(|t| slugify(t) == want) {
            hits.push(path);
        }
    }
    match hits.len() {
        1 => match hits.first() {
            Some(path) => safe_resolve(&cfg.spaces, path),
            None => Err(note_not_found(target)),
        },
        0 => Err(note_not_found(target).with_candidates(candidates(&paths, target))),
        _ => {
            let mut ids: Vec<String> = hits
                .iter()
                .filter_map(|p| meta_id(p).or_else(|| stem(p).map(str::to_string)))
                .collect();
            ids.sort();
            Err(MeshError::AmbiguousSlug {
                slug: target.to_string(),
                ids,
            })
        }
    }
}

fn note_not_found(target: &str) -> MeshError {
    MeshError::NoteNotFound(target.to_string())
}

/// Resolve to the note id, which is what names the lock. The id is the frontmatter's, so
/// an adopted file locks under its mesh id while keeping its foreign stem; a corrupt
/// `n-`-stemmed file falls back to the stem, which is the only name it has left.
fn resolve_id(cfg: &Config, target: &str) -> Result<String> {
    let path = resolve(cfg, target)?;
    meta_id(&path)
        .or_else(|| stem(&path).map(str::to_string))
        .ok_or_else(|| note_not_found(target))
}

/// The folder a note of this type lives in.
pub fn note_folder(cfg: &Config, note_type: &str) -> Result<PathBuf> {
    let root = cfg.root(Space::Notes)?;
    let sub = match note_type {
        "note" => return Ok(root.to_path_buf()),
        "log" => "logs",
        "decision" => "decisions",
        "reference" => "references",
        "project" => "projects",
        other => return Err(invalid_type(other)),
    };
    Ok(root.join(sub))
}

fn invalid_type(value: &str) -> MeshError {
    MeshError::Validation(format!("invalid note type: {value}"))
}

fn validate_type(value: &str) -> Result<()> {
    if NOTE_TYPES.contains(&value) {
        Ok(())
    } else {
        Err(invalid_type(value))
    }
}

// ---------------------------------------------------------------------------------------
// verbs
// ---------------------------------------------------------------------------------------

/// Create a note. Id allocation and the write both happen under the create lock.
pub fn create(cfg: &Config, title: &str, o: NewNote) -> Result<Note> {
    let note_type = if o.note_type.is_empty() {
        "note".to_string()
    } else {
        o.note_type.clone()
    };
    validate_type(&note_type)?;
    validate_owner(cfg, o.owner.as_deref())?;
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let folder = note_folder(cfg, &note_type)?;

    let _guard = hold(&create_lock(&root))?;
    let now = now_utc();
    // Both namespaces a minted id must not collide with: the frontmatter ids (adopted files
    // carry theirs in a foreign-stemmed name) and the `n-` stems (create names files by
    // id, so a stem collision would be a filename collision).
    let mut taken: Vec<String> = Vec::new();
    for path in mesh_paths(cfg) {
        if let Some(id) = meta_id(&path) {
            taken.push(id);
        }
        if is_mesh_stem(&path) {
            if let Some(s) = stem(&path) {
                taken.push(s.to_string());
            }
        }
    }
    let id = generate_id(NOTE_ID_PREFIX, &iso_z(&now), title, &|candidate| {
        taken.iter().any(|t| t == candidate)
    });

    let mut meta = Meta::new();
    meta.insert("id".to_string(), Value::str(id.as_str()));
    meta.insert("type".to_string(), Value::str(note_type.as_str()));
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
        Value::strings(resolve_wikilinks(cfg, &o.body)),
    );

    let path = safe_resolve(&cfg.spaces, &folder.join(format!("{id}.md")))?;
    let doc = Doc::new(meta, o.body);
    write_doc(&cfg.spaces, &path, &ordered(&NOTE_FIELDS, &doc))?;
    Note::from_meta(&doc.meta).ok_or(MeshError::NoteNotFound(id))
}

/// What `note adopt` committed: the mesh id now in the file's frontmatter, and the file
/// it lives in (which keeps its foreign stem).
#[derive(Clone, Debug)]
pub struct Adopted {
    pub id: String,
    pub path: PathBuf,
}

/// Where an adopt target lives: the path as given (absolute, or relative to the cwd), or
/// the same path read against the notes root or the vault root — the address forms
/// `note get --foreign` already accepts.
fn adopt_resolve(cfg: &Config, target: &Path) -> Option<PathBuf> {
    for base in [cfg.root(Space::Notes).ok()?, cfg.vault()] {
        let joined = base.join(target);
        if joined.is_file() {
            return Some(joined);
        }
    }
    target.is_file().then(|| target.to_path_buf())
}

/// A path rendered vault-relative when possible, absolute otherwise.
fn vault_rel(cfg: &Config, path: &Path) -> String {
    path.strip_prefix(cfg.vault())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Adopt existing foreign Markdown: mint a mesh id into each file, in place. Only the
/// keys mesh needs that the file lacks are added (`id`, `title`, `created`, `updated`);
/// the name, body and every foreign key survive untouched, and re-adoption is a
/// byte-identical no-op.
///
/// A batch is a sequence of single-entity transactions: the first failure stops the run
/// and names what already committed, and because adopt is idempotent, re-running the
/// same command heals.
///
/// `owner` stamps the long-term area the note belongs to, but only when the key is
/// absent: adoption never rewrites an area an operator or agent already recorded, and
/// the identity is validated at the write boundary as everywhere else.
pub fn adopt(cfg: &Config, targets: &[PathBuf], owner: Option<&str>) -> Result<Vec<Adopted>> {
    validate_owner(cfg, owner)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let mut adopted = Vec::new();
    for target in targets {
        match adopt_one(cfg, &root, target, owner) {
            Ok(one) => adopted.push(one),
            Err(e) => {
                if adopted.is_empty() {
                    return Err(e);
                }
                let committed = adopted
                    .iter()
                    .map(|a| format!("{} → {}", a.id, vault_rel(cfg, &a.path)))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(e.partial(format!(
                    "{committed} already adopted; re-run the same adopt command with the \
                     remaining paths to heal"
                )));
            }
        }
    }
    Ok(adopted)
}

/// Adopt one file. Sandbox first, then mint and write under the create lock with the file
/// re-read inside it: a racing adopt of the same file is either already done (a no-op
/// holding the same lock) or serialized behind this one.
fn adopt_one(cfg: &Config, root: &Path, target: &Path, owner: Option<&str>) -> Result<Adopted> {
    let missing = || note_not_found(&target.display().to_string());
    let Some(path) = adopt_resolve(cfg, target) else {
        return Err(missing());
    };
    let resolved = safe_resolve(&cfg.spaces, &path)?;
    // An adopt may touch a file inside the notes space that is not itself inside a nested
    // space root: another space's files are that space's to manage.
    let in_notes = resolved.strip_prefix(root).is_ok()
        && !cfg
            .spaces
            .exclusions_for(Space::Notes)
            .iter()
            .any(|excl| resolved.strip_prefix(excl).is_ok());
    if !in_notes {
        return Err(MeshError::validation(format!(
            "{} belongs to another space",
            target.display()
        )));
    }
    if !crate::storage::walk::walk_sees(root, &resolved) {
        return Err(MeshError::validation(format!(
            "{} cannot be adopted: mesh cannot see dot folders, non-.md files \
             or files over 4 MiB, so an id minted there would be unaddressable",
            target.display()
        )));
    }

    let _guard = hold(&create_lock(root))?;
    let text = std::fs::read_to_string(&resolved).map_err(|_| missing())?;
    let (yaml, body) = split_frontmatter(&text);
    let mut meta = match &yaml {
        None => Meta::new(),
        Some(block) => parse_meta(block).ok_or_else(missing)?,
    };
    if let Some(existing) = meta_str(&meta, "id") {
        if existing.starts_with(NOTE_ID_PREFIX) {
            return Ok(Adopted {
                id: existing.to_string(),
                path: resolved,
            });
        }
        return Err(MeshError::validation(format!(
            "foreign id: '{existing}' — remove or rename the id key to adopt {}",
            target.display()
        )));
    }
    let now = now_utc();
    let created = meta_time(&meta, "created").unwrap_or(now);
    let title = meta_str(&meta, "title")
        .map(str::to_string)
        .or_else(|| derived_title(&resolved, &body))
        .ok_or_else(missing)?;
    // The same two namespaces a minted id must not collide with as `create`: frontmatter
    // ids (adopted files carry theirs in a foreign stem) and `n-` stems.
    let mut taken: Vec<String> = Vec::new();
    for path in mesh_paths(cfg) {
        if let Some(id) = meta_id(&path) {
            taken.push(id);
        }
        if let Some(s) = stem(&path) {
            if s.starts_with(NOTE_ID_PREFIX) {
                taken.push(s.to_string());
            }
        }
    }
    let id = generate_id(NOTE_ID_PREFIX, &iso_z(&created), &title, &|candidate| {
        taken.iter().any(|t| t == candidate)
    });
    meta.insert("id".to_string(), Value::str(&id));
    if meta.get("title").is_none() {
        meta.insert("title".to_string(), Value::str(&title));
    }
    if meta.get("created").is_none() {
        meta.insert("created".to_string(), ts_value(&created));
    }
    if meta.get("updated").is_none() {
        meta.insert("updated".to_string(), ts_value(&now));
    }
    if let Some(owner) = owner {
        if meta.get("owner").is_none() {
            meta.insert("owner".to_string(), Value::str(owner));
        }
    }
    // Only absent keys are inserted, so a foreign value of the wrong shape survives: refuse
    // rather than mint an id that every read verb would then report as not found.
    if Note::from_meta(&meta).is_none() {
        return Err(MeshError::validation(format!(
            "{} fails the note schema: `tags` and `related` must be lists, `title`, `type`, \
             `owner` and `claimed_by` strings, `created` and `updated` timestamps — fix the \
             frontmatter, then adopt",
            target.display()
        )));
    }
    let doc = Doc::new(meta, body);
    write_doc(&cfg.spaces, &resolved, &ordered(&NOTE_FIELDS, &doc))?;
    Ok(Adopted { id, path: resolved })
}

/// Recompute `related` from the current body and bump `updated`. Both keys are overwritten
/// wholesale, in place, so a Python-era file keeps its own key order.
fn restamp(cfg: &Config, doc: &mut Doc) {
    let related = resolve_wikilinks(cfg, &doc.body);
    doc.meta
        .insert("related".to_string(), Value::strings(related));
    doc.meta.insert("updated".to_string(), ts_value(&now_utc()));
}

/// Append a block to a note's body, optionally under `## {section}` and timestamped.
pub fn append(cfg: &Config, target: &str, text: &str, o: AppendOpts) -> Result<Note> {
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let actor = o.actor.clone().or_else(|| cfg.agent().map(str::to_string));
    let block = format_block(text, o.timestamp, actor.as_deref());

    let _guard = hold(&entity_lock(&root, &note_id)?)?;
    let path = resolve(cfg, &note_id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(note_not_found(target));
    };
    doc.body = match o.section.as_deref() {
        Some(section) => append_under_section(&doc.body, &block, section),
        None => append_to_end(&doc.body, &block),
    };
    restamp(cfg, &mut doc);
    let note = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;
    write_doc(&cfg.spaces, &path, &ordered(&NOTE_FIELDS, &doc))?;
    Ok(note)
}

/// Update a note's tags, type or title. A type change moves the file inside the lock.
pub fn update(cfg: &Config, target: &str, o: UpdateNote) -> Result<Note> {
    if let Some(new_type) = &o.new_type {
        validate_type(new_type)?;
    }
    validate_owner(cfg, o.owner.as_deref())?;
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();

    let _guard = hold(&entity_lock(&root, &note_id)?)?;
    let path = resolve(cfg, &note_id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(note_not_found(target));
    };
    if let Some(spec) = &o.tags {
        let next = apply_tag_spec(&meta_strings(&doc.meta, "tags"), spec)?;
        doc.meta.insert("tags".to_string(), Value::strings(next));
    }
    if let Some(new_type) = &o.new_type {
        doc.meta
            .insert("type".to_string(), Value::str(new_type.as_str()));
    }
    if let Some(title) = &o.title {
        doc.meta
            .insert("title".to_string(), Value::str(title.as_str()));
    }
    // An explicit `--owner` set: changing an area is an update action, never an
    // adoption side effect.
    if let Some(owner) = &o.owner {
        doc.meta.insert("owner".to_string(), Value::str(owner));
    }
    restamp(cfg, &mut doc);
    let note = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;

    // Move first, then write. Reads are frontmatter-driven, so a failure has to leave the
    // frontmatter describing the state on disk: write-then-move strands a note whose
    // frontmatter already says `log` in the folder for its old type, and nothing reports it.
    // Moving first means a failed rename changes nothing at all, and a failed write leaves
    // the *old* content — which is what `note get` then truthfully reports. The folder is
    // then the only thing out of step, and the repair below heals it on the next update.
    // Two reasons to relocate, and nothing else: the caller changed `--type`, or the note is
    // sitting directly in the space root when its type says otherwise — the shape an
    // interrupted move leaves behind, which the next update heals.
    //
    // The operator owns the vault's folder layout. A note they filed under
    // `notes/archive/2026/` is not misfiled, it is organised, so a `--title` edit must leave
    // it exactly where it is. Relocating on every update flattened that silently.
    let stranded_at_root = path.parent() == Some(root.as_path());
    let dest = if o.new_type.is_some() || stranded_at_root {
        destination(cfg, &path, &doc)?
    } else {
        path.clone()
    };
    if dest != path {
        if dest.exists() {
            return Err(MeshError::Validation(format!(
                "cannot file {note_id} as {}: {} already exists",
                meta_str(&doc.meta, "type").unwrap_or("note"),
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&path, &dest)?;
    }
    write_doc(&cfg.spaces, &dest, &ordered(&NOTE_FIELDS, &doc))?;
    Ok(note)
}

/// Where a note belongs given the frontmatter it is about to carry.
///
/// This runs on **every** update, not only a `--type` change, so a note stranded in the wrong
/// folder by an interrupted earlier update is healed by the next one — the same idempotent
/// repair `tasks::terminate` performs through `move_if_needed`. An unknown type keeps the
/// note where it is rather than failing the update.
fn destination(cfg: &Config, path: &Path, doc: &Doc) -> Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return Ok(path.to_path_buf());
    };
    let Some(note_type) = meta_str(&doc.meta, "type") else {
        return Ok(path.to_path_buf());
    };
    let Ok(folder) = note_folder(cfg, note_type) else {
        return Ok(path.to_path_buf());
    };
    safe_resolve(&cfg.spaces, &folder.join(name))
}

/// Read one note: frontmatter, body and path.
pub fn get(cfg: &Config, target: &str) -> Result<View<Note>> {
    let path = resolve(cfg, target)?;
    let Some(doc) = read_doc(&path) else {
        return Err(note_not_found(target));
    };
    let item = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;
    Ok(View {
        item,
        body: doc.body,
        path,
    })
}

/// The first `# H1` in a body, else the filename stem.
fn derived_title(path: &Path, body: &str) -> Option<String> {
    for line in body.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("# ") {
            let text = rest.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    stem(path).map(str::to_string)
}

fn foreign_view(path: &Path) -> ForeignView {
    let body = read_body(path);
    ForeignView {
        title: derived_title(path, &body),
        body,
        path: path.to_path_buf(),
    }
}

/// Read a non-mesh Markdown file by stem, notes-relative path or vault-relative path.
pub fn get_foreign(cfg: &Config, target: &str) -> Result<ForeignView> {
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let vault = cfg.vault().to_path_buf();
    for path in foreign_paths(cfg) {
        let rel = |base: &Path| -> Option<String> {
            path.strip_prefix(base)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        };
        if stem(&path) == Some(target)
            || rel(&root).as_deref() == Some(target)
            || rel(&vault).as_deref() == Some(target)
        {
            return Ok(foreign_view(&path));
        }
    }
    Err(note_not_found(target))
}

/// How many non-mesh Markdown files live in the notes space — the half of the corpus
/// `status` names beside the mesh-native count, so `notes: 0` on a vault full of foreign
/// files cannot read as "mesh sees nothing".
pub fn foreign_count(cfg: &Config) -> usize {
    foreign_paths(cfg).len()
}

/// The foreign file a lens seed names, when one matches: [`get_foreign`]'s matcher
/// (stem, notes-relative path, vault-relative path) plus a slug match over the stem and
/// the derived title. `None` means "no such file" — what the seed error must keep
/// distinguishing from "exists, but mesh did not author it".
pub fn find_foreign(cfg: &Config, target: &str) -> Option<PathBuf> {
    if let Ok(view) = get_foreign(cfg, target) {
        return Some(view.path);
    }
    let want = slugify(target);
    if want.is_empty() {
        return None;
    }
    foreign_paths(cfg).into_iter().find(|path| {
        stem(path).is_some_and(|s| slugify(s) == want)
            || derived_title(path, &read_body(path)).is_some_and(|title| slugify(&title) == want)
    })
}

/// List mesh notes: id-bearing, schema-valid files only.
///
/// `foreign` cannot be honoured through this return type — a `View<Note>` has no shape for a
/// file with no frontmatter — so `note list --foreign` reads its extra rows from
/// [`foreign_rows`] and concatenates them.
pub fn list(cfg: &Config, f: &Filter, foreign: bool) -> Result<Vec<View<Note>>> {
    let _ = foreign;
    cfg.root(Space::Notes)?;
    Ok(select(rows(cfg), f))
}

/// The foreign Markdown a `--foreign` listing adds, in sorted path order.
///
/// A foreign file carries no tags, owner, type or `updated`, so it is admitted only when the
/// filter asks for none of them — the same conjunctive rule `select` applies, evaluated here
/// against empty frontmatter.
pub fn foreign_rows(cfg: &Config, f: &Filter) -> Vec<ForeignView> {
    if !matches_filters(&Meta::new(), f) {
        return Vec::new();
    }
    foreign_paths(cfg).iter().map(|p| foreign_view(p)).collect()
}

/// Atomically take a claim on a note: a test-and-set on `claimed_by` alone.
///
/// Mirrors `tasks::claim` minus the lifecycle: a note gains no status and the claim never
/// moves the file. Resolution happens twice — once before the lock so a missing note is
/// not-found rather than a conflict, and again *inside* the lock, which is the TOCTOU rule
/// every notes verb follows. A re-claim by the same identity is a no-op that reports the
/// state it found and never rewrites; a claim by a second identity exits 4 with the task
/// claim conflict envelope.
pub fn claim(cfg: &Config, target: &str, claimer: &str) -> Result<Note> {
    // Resolve first, so a missing note is not-found rather than "unclaimed".
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();

    let _guard = hold(&entity_lock(&root, &note_id)?)?;
    let path = resolve(cfg, &note_id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(note_not_found(target));
    };
    let existing = meta_str(&doc.meta, "claimed_by").map(str::to_string);
    match existing {
        // Same-identity reclaim: no write, `updated` untouched.
        Some(ref who) if who == claimer => {
            Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))
        }
        // Someone else holds it.
        Some(who) => Err(MeshError::ClaimConflict {
            noun: "note",
            task_id: note_id,
            existing_owner: who,
        }),
        // Take it. `owner` and every other field are untouched.
        None => {
            doc.meta
                .insert("claimed_by".to_string(), Value::str(claimer));
            let now = now_utc();
            doc.meta.insert("updated".to_string(), ts_value(&now));
            let note = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;
            write_doc(&cfg.spaces, &path, &ordered(&NOTE_FIELDS, &doc))?;
            Ok(note)
        }
    }
}

/// Release a claim on a note: clear `claimed_by`.
///
/// Mirrors `tasks::release` minus the lifecycle: idempotent, a no-op release never rewrites,
/// `force` breaks another holder's claim, and the cleared key is emitted as `null`. `owner`
/// is never touched.
pub fn release(cfg: &Config, target: &str, releaser: &str, force: bool) -> Result<Note> {
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();

    let _guard = hold(&entity_lock(&root, &note_id)?)?;
    let path = resolve(cfg, &note_id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(note_not_found(target));
    };
    let Some(holder) = meta_str(&doc.meta, "claimed_by").map(str::to_string) else {
        // Releasing an unclaimed note is an idempotent no-op.
        return Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target));
    };
    // `force` is a cooperation override and an audit affordance, never an auth check.
    if holder != releaser && !force {
        return Err(MeshError::ClaimConflict {
            noun: "note",
            task_id: note_id,
            existing_owner: holder,
        });
    }
    doc.meta.insert("claimed_by".to_string(), Value::Null);
    let now = now_utc();
    doc.meta.insert("updated".to_string(), ts_value(&now));
    let note = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;
    write_doc(&cfg.spaces, &path, &ordered(&NOTE_FIELDS, &doc))?;
    Ok(note)
}

/// Hard-delete a note. Removing a file with corrupt frontmatter is the repair path, so this
/// is the one verb that never validates.
pub fn delete(cfg: &Config, target: &str) -> Result<String> {
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, &note_id)?)?;
    let path = resolve(cfg, &note_id)?;
    std::fs::remove_file(&path)?;
    Ok(note_id)
}

/// Every readable `(path, frontmatter)` pair in the notes space — the scan every listing,
/// lens and index pass shares. Unreadable and unparseable files are skipped silently.
pub fn rows(cfg: &Config) -> Vec<Row> {
    all_paths(cfg)
        .into_iter()
        .filter_map(|path| read_meta_only(&path).map(|meta| Row { path, meta }))
        .collect()
}

/// The id of the first note whose slugified title matches `title`, if any.
///
/// Slug-normalised and notes-only, so `Japan Visa` collides with `japan  visa` but never
/// with a task or a memory of the same name. Advisory: the caller still creates the note.
pub fn find_duplicate_title(cfg: &Config, title: &str) -> Option<String> {
    let want = slugify(title);
    rows(cfg).into_iter().find_map(|row| {
        let id = meta_str(&row.meta, "id")?;
        if !id.starts_with(NOTE_ID_PREFIX) {
            return None;
        }
        if slugify(meta_str(&row.meta, "title")?) == want {
            Some(id.to_string())
        } else {
            None
        }
    })
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

    struct Vault {
        _dir: tempfile::TempDir,
        cfg: Config,
    }

    fn vault() -> Vault {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        Vault { _dir: dir, cfg }
    }

    fn write(cfg: &Config, rel: &str, text: &str) {
        let path = cfg.vault().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn note_body(id: &str, title: &str, note_type: &str) -> String {
        format!(
            "---\nid: {id}\ntype: {note_type}\ntitle: {title}\ntags: []\nowner: null\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
             ---\n\nbody\n"
        )
    }

    #[test]
    fn create_routes_by_type_and_allocates_an_id() {
        let v = vault();
        for (note_type, folder) in [
            ("note", "notes"),
            ("log", "notes/logs"),
            ("decision", "notes/decisions"),
            ("reference", "notes/references"),
            ("project", "notes/projects"),
        ] {
            let note = create(
                &v.cfg,
                &format!("T {note_type}"),
                NewNote {
                    note_type: note_type.to_string(),
                    ..NewNote::default()
                },
            )
            .unwrap();
            assert!(note.id.starts_with("n-"));
            assert_eq!(note.note_type, note_type);
            let expected = v.cfg.vault().join(folder).join(format!("{}.md", note.id));
            assert!(expected.is_file(), "{}", expected.display());
        }
    }

    #[test]
    fn create_rejects_an_unknown_type_before_writing() {
        let v = vault();
        let err = create(
            &v.cfg,
            "T",
            NewNote {
                note_type: "memo".into(),
                ..NewNote::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "invalid note type: memo");
        assert!(rows(&v.cfg).is_empty());
    }

    #[test]
    fn create_derives_related_from_wikilinks() {
        let v = vault();
        let target = create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let linker = create(
            &v.cfg,
            "Linker",
            NewNote {
                body: "see [[Alpha]] and [[n-9999]] and [[Ghost]]".into(),
                ..NewNote::default()
            },
        )
        .unwrap();
        assert_eq!(linker.related, [target.id.as_str(), "n-9999"]);
    }

    #[test]
    fn resolution_accepts_an_id_or_a_slug() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-AAAA.md",
            &note_body("n-AAAA", "My Title", "note"),
        );
        assert_eq!(resolve_id(&v.cfg, "n-AAAA").unwrap(), "n-AAAA");
        assert_eq!(resolve_id(&v.cfg, "My Title").unwrap(), "n-AAAA");
        assert_eq!(resolve_id(&v.cfg, "my--title").unwrap(), "n-AAAA");
    }

    #[test]
    fn an_ambiguous_slug_lists_sorted_ids() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-BBBB.md",
            &note_body("n-BBBB", "Dupe", "note"),
        );
        write(
            &v.cfg,
            "notes/logs/n-AAAA.md",
            &note_body("n-AAAA", "Dupe", "log"),
        );
        let err = resolve(&v.cfg, "dupe").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "ambiguous slug 'dupe': n-AAAA, n-BBBB");
    }

    #[test]
    fn a_miss_carries_candidates() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-AAAA.md",
            &note_body("n-AAAA", "Japan Visa", "note"),
        );
        let err = resolve(&v.cfg, "japan-visas").unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "note not found: japan-visas");
        assert_eq!(err.candidates(), ["n-AAAA"]);
    }

    #[test]
    fn foreign_files_never_resolve_and_never_list() {
        let v = vault();
        write(&v.cfg, "notes/loose.md", "# Loose Heading\n\ntext\n");
        assert_eq!(resolve(&v.cfg, "loose").unwrap_err().code(), 3);
        assert_eq!(resolve(&v.cfg, "Loose Heading").unwrap_err().code(), 3);
        assert!(list(&v.cfg, &Filter::unbounded(), true).unwrap().is_empty());
        let foreign = foreign_rows(&v.cfg, &Filter::unbounded());
        assert_eq!(foreign.len(), 1);
        assert_eq!(foreign[0].title.as_deref(), Some("Loose Heading"));
        assert_eq!(
            get_foreign(&v.cfg, "loose").unwrap().body,
            "# Loose Heading\n\ntext"
        );
    }

    #[test]
    fn a_foreign_file_without_an_h1_falls_back_to_its_stem() {
        let v = vault();
        write(&v.cfg, "notes/plain.md", "just text\n");
        let view = get_foreign(&v.cfg, "notes/plain.md").unwrap();
        assert_eq!(view.title.as_deref(), Some("plain"));
    }

    #[test]
    fn a_filtered_listing_never_admits_foreign_rows() {
        let v = vault();
        write(&v.cfg, "notes/loose.md", "# Loose\n");
        let filtered = Filter::unbounded().with_extra("type", Some("note"));
        assert!(foreign_rows(&v.cfg, &filtered).is_empty());
        assert_eq!(foreign_rows(&v.cfg, &Filter::unbounded()).len(), 1);
    }

    #[test]
    fn a_corrupt_note_is_not_found_but_still_deletable() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-BAD.md",
            "---\nid: n-BAD\ntitle: [unclosed\n---\n\nbroken\n",
        );
        assert_eq!(get(&v.cfg, "n-BAD").unwrap_err().code(), 3);
        assert_eq!(
            append(&v.cfg, "n-BAD", "x", AppendOpts::default())
                .unwrap_err()
                .code(),
            3
        );
        assert_eq!(delete(&v.cfg, "n-BAD").unwrap(), "n-BAD");
        assert!(!v.cfg.vault().join("notes/n-BAD.md").exists());
    }

    #[test]
    fn append_bumps_updated_and_keeps_created() {
        let v = vault();
        let note = create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let after = append(&v.cfg, &note.id, "more", AppendOpts::default()).unwrap();
        assert_eq!(after.created, note.created);
        assert!(after.updated >= note.updated);
        let body = read_doc(&resolve(&v.cfg, &note.id).unwrap()).unwrap().body;
        assert_eq!(body, "more");
    }

    #[test]
    fn append_under_a_section_creates_it_when_absent() {
        let v = vault();
        let note = create(
            &v.cfg,
            "Alpha",
            NewNote {
                body: "Intro.\n\n## A\n\nitem1".into(),
                ..NewNote::default()
            },
        )
        .unwrap();
        append(
            &v.cfg,
            &note.id,
            "NEW",
            AppendOpts {
                section: Some("A".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        let path = resolve(&v.cfg, &note.id).unwrap();
        assert_eq!(
            read_doc(&path).unwrap().body,
            "Intro.\n\n## A\n\nitem1\n\nNEW"
        );
        append(
            &v.cfg,
            &note.id,
            "Z",
            AppendOpts {
                section: Some("Zed".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        assert!(read_doc(&path).unwrap().body.ends_with("## Zed\n\nZ"));
    }

    #[test]
    fn a_timestamped_append_stamps_the_body_not_the_frontmatter() {
        let v = vault();
        let note = create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let after = append(
            &v.cfg,
            &note.id,
            "line",
            AppendOpts {
                timestamp: true,
                actor: Some("agent-x".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        let body = read_doc(&resolve(&v.cfg, &note.id).unwrap()).unwrap().body;
        assert!(body.contains(" — agent-x\nline"), "{body}");
        assert!(!after.meta.contains_key("actor"));
    }

    #[test]
    fn update_moves_the_file_and_keeps_the_filename() {
        let v = vault();
        let note = create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let moved = update(
            &v.cfg,
            &note.id,
            UpdateNote {
                new_type: Some("log".into()),
                ..UpdateNote::default()
            },
        )
        .unwrap();
        assert_eq!(moved.note_type, "log");
        assert!(!v.cfg.vault().join(format!("notes/{}.md", note.id)).exists());
        assert!(v
            .cfg
            .vault()
            .join(format!("notes/logs/{}.md", note.id))
            .is_file());
    }

    #[test]
    fn update_applies_the_tag_grammar_and_rejects_a_mixed_spec() {
        let v = vault();
        let note = create(&v.cfg, "Alpha", NewNote::default()).unwrap();
        let tagged = update(
            &v.cfg,
            &note.id,
            UpdateNote {
                tags: Some("a,b".into()),
                ..UpdateNote::default()
            },
        )
        .unwrap();
        assert_eq!(tagged.tags, ["a", "b"]);
        let err = update(
            &v.cfg,
            &note.id,
            UpdateNote {
                tags: Some("+c,d".into()),
                ..UpdateNote::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(get(&v.cfg, &note.id).unwrap().item.tags, ["a", "b"]);
    }

    #[test]
    fn unknown_keys_round_trip_through_an_amend() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-HAND.md",
            "---\nid: n-HAND\ntitle: Hand\ncustom_key: keep me\nextra:\n  nested: yes\n\
             created: 2026-01-02\nupdated: 2026-01-02T03:04:05\n---\n\nbody\n",
        );
        append(&v.cfg, "n-HAND", "more", AppendOpts::default()).unwrap();
        let doc = read_doc(&v.cfg.vault().join("notes/n-HAND.md")).unwrap();
        assert_eq!(meta_str(&doc.meta, "custom_key"), Some("keep me"));
        assert!(doc.meta.contains_key("extra"));
        assert!(doc.meta.contains_key("related"));
    }

    #[test]
    fn duplicate_titles_are_slug_normalised_and_notes_only() {
        let v = vault();
        let first = create(&v.cfg, "Japan Visa", NewNote::default()).unwrap();
        assert_eq!(
            find_duplicate_title(&v.cfg, "  japan   visa!"),
            Some(first.id.clone())
        );
        assert_eq!(find_duplicate_title(&v.cfg, "Something Else"), None);
        write(
            &v.cfg,
            "tasks/open/t-AAAA.md",
            "---\nid: t-AAAA\ntype: task\ntitle: Japan Visa\n\
             created: 2026-01-02\nupdated: 2026-01-02\n---\n\nx\n",
        );
        assert_eq!(
            find_duplicate_title(&v.cfg, "japan visa"),
            Some(first.id),
            "a task never collides with a note"
        );
    }

    #[test]
    fn the_owner_roster_is_enforced_at_the_write_boundary() {
        let mut v = vault();
        v.cfg.tasks.collections = vec!["alice".into()];
        let err = create(
            &v.cfg,
            "T",
            NewNote {
                owner: Some("ghost".into()),
                ..NewNote::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "unknown owner: 'ghost'");
        assert!(rows(&v.cfg).is_empty());
    }

    #[test]
    fn note_folder_maps_every_type() {
        let v = vault();
        let root = v.cfg.root(Space::Notes).unwrap().to_path_buf();
        assert_eq!(note_folder(&v.cfg, "note").unwrap(), root);
        assert_eq!(note_folder(&v.cfg, "log").unwrap(), root.join("logs"));
        assert_eq!(
            note_folder(&v.cfg, "decision").unwrap(),
            root.join("decisions")
        );
        assert_eq!(
            note_folder(&v.cfg, "reference").unwrap(),
            root.join("references")
        );
        assert_eq!(
            note_folder(&v.cfg, "project").unwrap(),
            root.join("projects")
        );
        assert!(note_folder(&v.cfg, "memo").is_err());
    }

    #[test]
    fn an_id_bearing_file_with_a_foreign_stem_and_type_lists_and_resolves() {
        let v = vault();
        write(
            &v.cfg,
            "notes/team-sol.md",
            "---\nid: n-SOL1\ntype: Team\ntitle: Team Sol\nbelongs_to: []\nschema: 3\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-03T00:00:00Z\n---\n\n# Team Sol\n",
        );
        let listed = list(&v.cfg, &Filter::unbounded(), false).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].item.id, "n-SOL1");
        assert_eq!(listed[0].item.note_type, "Team");
        assert_eq!(resolve_id(&v.cfg, "n-SOL1").unwrap(), "n-SOL1");
        assert_eq!(resolve_id(&v.cfg, "Team Sol").unwrap(), "n-SOL1");
        let got = get(&v.cfg, "n-SOL1").unwrap();
        assert_eq!(got.item.title, "Team Sol");
        assert_eq!(got.path, v.cfg.vault().join("notes/team-sol.md"));
        assert!(
            got.item.meta.contains_key("schema") && got.item.meta.contains_key("belongs_to"),
            "unknown keys ride along untouched"
        );
    }

    #[test]
    fn classification_is_by_frontmatter_id_not_by_stem() {
        let v = vault();
        // The discriminating case: under stem classification both files are foreign (2);
        // under id classification the id-bearing file has left the foreign set (1).
        write(&v.cfg, "notes/loose.md", "# Loose\n");
        write(
            &v.cfg,
            "notes/team-sol.md",
            "---\nid: n-SOL1\ntype: Team\ntitle: Team Sol\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-03T00:00:00Z\n---\n\nx\n",
        );
        assert_eq!(foreign_count(&v.cfg), 1, "only the id-less file is foreign");
    }

    #[test]
    fn delete_removes_an_adopted_shaped_file_by_meta_id() {
        let v = vault();
        write(
            &v.cfg,
            "notes/team-sol.md",
            "---\nid: n-SOL1\ntype: Team\ntitle: Team Sol\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-03T00:00:00Z\n---\n\nx\n",
        );
        assert_eq!(delete(&v.cfg, "n-SOL1").unwrap(), "n-SOL1");
        assert!(!v.cfg.vault().join("notes/team-sol.md").exists());
    }

    #[test]
    fn a_miss_carries_candidates_from_foreign_stems() {
        let v = vault();
        write(
            &v.cfg,
            "notes/team-sol.md",
            "---\nid: n-SOL1\ntype: Team\ntitle: Team Sol\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-03T00:00:00Z\n---\n\nx\n",
        );
        let err = resolve(&v.cfg, "n-SOLX").unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.candidates(), ["n-SOL1"]);
    }

    #[test]
    fn rows_skip_unparseable_files_but_keep_foreign_ones() {
        let v = vault();
        write(&v.cfg, "notes/n-AAAA.md", &note_body("n-AAAA", "A", "note"));
        write(
            &v.cfg,
            "notes/n-BAD.md",
            "---\ntitle: [unclosed\n---\n\nx\n",
        );
        write(&v.cfg, "notes/loose.md", "# Loose\n");
        let rows = rows(&v.cfg);
        assert_eq!(rows.len(), 2, "the unparseable file is skipped");
        let listed = list(&v.cfg, &Filter::unbounded(), false).unwrap();
        assert_eq!(listed.len(), 1);
    }

    // ---------------------------------------------------------------------------------
    // adopt
    // ---------------------------------------------------------------------------------

    const FOREIGN_TEAM: &str = "---\ntype: Team\ntitle: Team Sol\nbelongs_to: []\nschema: 3\n\
                                 status: captured\n---\n\n# Team Sol\n\nBody text.\n";

    #[test]
    fn adopt_mints_the_minimal_mesh_block() {
        let v = vault();
        write(&v.cfg, "notes/team-sol.md", FOREIGN_TEAM);
        let adopted = adopt(&v.cfg, &[v.cfg.vault().join("notes/team-sol.md")], None).unwrap();
        assert_eq!(adopted.len(), 1);
        let id = adopted[0].id.clone();
        assert!(id.starts_with(NOTE_ID_PREFIX));

        // Only the four mesh keys are injected; every foreign key survives in place.
        let Some(doc) = read_doc(&v.cfg.vault().join("notes/team-sol.md")) else {
            panic!("the adopted file must be readable");
        };
        assert_eq!(meta_str(&doc.meta, "id"), Some(id.as_str()));
        assert_eq!(meta_str(&doc.meta, "type"), Some("Team"));
        assert_eq!(meta_str(&doc.meta, "title"), Some("Team Sol"));
        assert_eq!(meta_str(&doc.meta, "status"), Some("captured"));
        assert!(doc.meta.contains_key("belongs_to"));
        assert!(doc.meta.contains_key("schema"));
        assert!(meta_time(&doc.meta, "created").is_some());
        assert!(meta_time(&doc.meta, "updated").is_some());
        assert_eq!(doc.body, "# Team Sol\n\nBody text.");

        // The file keeps its foreign stem and is now addressable by id.
        assert!(adopted[0].path.ends_with("team-sol.md"));
        let note = get(&v.cfg, &id).unwrap();
        assert_eq!(note.item.title, "Team Sol");
        assert_eq!(note.item.note_type, "Team");
        assert_eq!(list(&v.cfg, &Filter::unbounded(), false).unwrap().len(), 1);
    }

    #[test]
    fn adopt_preserves_and_reuses_an_existing_created() {
        let v = vault();
        write(
            &v.cfg,
            "notes/pinned.md",
            "---\ntitle: Pinned\ncreated: 2026-01-02\n---\n\n# Pinned\n",
        );
        let adopted = adopt(&v.cfg, &[v.cfg.vault().join("notes/pinned.md")], None).unwrap();
        let id = adopted[0].id.clone();
        // The existing created is the digest input, so the id is reproducible.
        let expected = generate_id(NOTE_ID_PREFIX, "2026-01-02T00:00:00Z", "Pinned", &|_| false);
        assert_eq!(id, expected);
        let text = std::fs::read_to_string(v.cfg.vault().join("notes/pinned.md")).unwrap();
        assert!(
            text.contains("created: 2026-01-02\n"),
            "raw preserved: {text}"
        );
        assert!(text.contains("updated: 2"), "updated is stamped: {text}");
    }

    #[test]
    fn re_adoption_is_a_byte_identical_noop() {
        let v = vault();
        write(&v.cfg, "notes/team-sol.md", FOREIGN_TEAM);
        let target = v.cfg.vault().join("notes/team-sol.md");
        let first = adopt(&v.cfg, std::slice::from_ref(&target), None).unwrap();
        let before = std::fs::read_to_string(&target).unwrap();
        let second = adopt(&v.cfg, std::slice::from_ref(&target), None).unwrap();
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), before);
    }

    #[test]
    fn adopt_prepends_a_block_to_a_frontmatter_less_file() {
        let v = vault();
        write(&v.cfg, "notes/loose.md", "# Just a body\n\ntext\n");
        let adopted = adopt(&v.cfg, &[v.cfg.vault().join("notes/loose.md")], None).unwrap();
        let id = adopted[0].id.clone();
        let Some(doc) = read_doc(&v.cfg.vault().join("notes/loose.md")) else {
            panic!("the adopted file must be readable");
        };
        assert_eq!(meta_str(&doc.meta, "id"), Some(id.as_str()));
        assert_eq!(meta_str(&doc.meta, "title"), Some("Just a body"));
        assert_eq!(doc.body, "# Just a body\n\ntext");
        let raw = std::fs::read_to_string(v.cfg.vault().join("notes/loose.md")).unwrap();
        assert!(
            raw.contains("# Just a body\n\ntext\n") && raw.ends_with('\n'),
            "body intact, one trailing newline: {raw}"
        );
    }

    #[test]
    fn adopt_refuses_a_malformed_block_and_writes_nothing() {
        let v = vault();
        let text = "---\ntitle: [unclosed\n---\n\nbroken\n";
        write(&v.cfg, "notes/broken.md", text);
        let target = v.cfg.vault().join("notes/broken.md");
        let err = adopt(&v.cfg, std::slice::from_ref(&target), None).unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
    }

    #[test]
    fn adopt_refuses_a_foreign_id_key() {
        let v = vault();
        let text = "---\nid: mine-1\ntitle: Owned elsewhere\n---\n\nx\n";
        write(&v.cfg, "notes/owned.md", text);
        let target = v.cfg.vault().join("notes/owned.md");
        let err = adopt(&v.cfg, std::slice::from_ref(&target), None).unwrap_err();
        assert_eq!(err.code(), 2);
        let msg = err.to_string();
        assert!(msg.contains("foreign id"), "{msg}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), text);
    }

    #[test]
    fn adopt_reports_a_missing_path_as_not_found() {
        let v = vault();
        let err = adopt(&v.cfg, &[v.cfg.vault().join("notes/nope.md")], None).unwrap_err();
        assert_eq!(err.code(), 3);
    }

    #[test]
    fn adopt_refuses_paths_outside_the_notes_space() {
        let v = vault();
        // Inside the vault, but in another space: not adoptable as a note.
        write(&v.cfg, "tasks/other-space.md", "---\ntitle: X\n---\n\nx\n");
        let err = adopt(&v.cfg, &[v.cfg.vault().join("tasks/other-space.md")], None).unwrap_err();
        assert_eq!(err.code(), 2);
        // Outside the vault entirely: sandbox escape, refused.
        let outside = tempfile::tempdir().unwrap();
        let foreign = outside.path().join("elsewhere.md");
        std::fs::write(&foreign, "# Elsewhere\n").unwrap();
        let err = adopt(&v.cfg, &[foreign], None).unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn an_id_collision_extends_the_id() {
        let v = vault();
        let text = "---\ntitle: Same\ncreated: 2026-01-02\n---\n\n# Same\n";
        write(&v.cfg, "notes/one.md", text);
        write(&v.cfg, "notes/two.md", text);
        let adopted = adopt(
            &v.cfg,
            &[
                v.cfg.vault().join("notes/one.md"),
                v.cfg.vault().join("notes/two.md"),
            ],
            None,
        )
        .unwrap();
        assert_ne!(adopted[0].id, adopted[1].id);
    }

    #[test]
    fn adopt_stops_at_the_first_failure_and_names_what_committed() {
        let v = vault();
        write(&v.cfg, "notes/good.md", FOREIGN_TEAM);
        let good = v.cfg.vault().join("notes/good.md");
        let missing = v.cfg.vault().join("notes/nope.md");
        let err = adopt(&v.cfg, &[good.clone(), missing], None).unwrap_err();
        assert_eq!(err.code(), 3);
        let msg = err.to_string();
        assert!(msg.contains("re-run"), "{msg}");
        let committed_id = meta_id(&good).expect("good committed");
        assert!(msg.contains(committed_id.as_str()), "{msg}");
        assert!(meta_id(&good).is_some(), "the first file did commit");
    }

    const FOREIGN_OWNERED: &str = "---\ntype: Team\ntitle: Owned\nowner: bob\n---\n\n# Owned\n";

    #[test]
    fn adopt_inserts_the_owner_only_when_given_and_absent() {
        let v = vault();
        write(&v.cfg, "notes/bare.md", FOREIGN_TEAM);
        write(&v.cfg, "notes/owned.md", FOREIGN_OWNERED);
        write(&v.cfg, "notes/quiet.md", FOREIGN_TEAM);

        // Given and absent: inserted.
        adopt(
            &v.cfg,
            &[v.cfg.vault().join("notes/bare.md")],
            Some("alice"),
        )
        .unwrap();
        let doc = read_doc(&v.cfg.vault().join("notes/bare.md")).expect("readable");
        assert_eq!(meta_str(&doc.meta, "owner"), Some("alice"));

        // Given but the key is present: untouched — changing an area is an explicit
        // `note update`, never an adoption side effect.
        adopt(
            &v.cfg,
            &[v.cfg.vault().join("notes/owned.md")],
            Some("alice"),
        )
        .unwrap();
        let doc = read_doc(&v.cfg.vault().join("notes/owned.md")).expect("readable");
        assert_eq!(meta_str(&doc.meta, "owner"), Some("bob"));

        // Flag absent: the key is never injected.
        adopt(&v.cfg, &[v.cfg.vault().join("notes/quiet.md")], None).unwrap();
        let doc = read_doc(&v.cfg.vault().join("notes/quiet.md")).expect("readable");
        assert!(!doc.meta.contains_key("owner"));
    }

    #[test]
    fn update_owner_is_an_explicit_set() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        let updated = update(
            &v.cfg,
            &note.id,
            UpdateNote {
                owner: Some("bob".to_string()),
                ..UpdateNote::default()
            },
        )
        .unwrap();
        assert_eq!(updated.owner.as_deref(), Some("bob"));
        let doc = read_doc(&v.cfg.vault().join("notes").join(format!("{}.md", note.id)))
            .expect("readable");
        assert_eq!(meta_str(&doc.meta, "owner"), Some("bob"));
    }

    // ---------------------------------------------------------------------------------
    // claim / release (note-adoption/4)
    // ---------------------------------------------------------------------------------

    /// The note's file path, whether it lives at the space root or a typed folder.
    fn note_path(v: &Vault, id: &str) -> PathBuf {
        let path = resolve(&v.cfg, id).unwrap();
        assert!(path.is_file(), "{}", path.display());
        path
    }

    #[test]
    fn claim_writes_claimed_by_only_and_leaves_owner_alone() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        let path = note_path(&v, &note.id);
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("owner: test-agent\n"), "{before}");
        assert!(!before.contains("claimed_by"), "{before}");

        let claimed = claim(&v.cfg, &note.id, "alice").unwrap();
        assert_eq!(claimed.claimed_by.as_deref(), Some("alice"));
        // The claim never touches the durable owner, and a note never gains a status.
        assert_eq!(claimed.owner.as_deref(), Some("test-agent"));

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("claimed_by: alice\n"), "{after}");
        assert!(after.contains("owner: test-agent\n"), "{after}");
        assert!(
            !after.contains("status:"),
            "a note never gains a status: {after}"
        );
        assert_ne!(before, after, "the claim is a write");

        // claimed_by is declared after owner, so it lands between owner and created.
        let keys: Vec<&str> = after
            .lines()
            .skip(1)
            .take_while(|l| *l != "---")
            .filter_map(|l| l.split(':').next())
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "type",
                "title",
                "tags",
                "owner",
                "claimed_by",
                "created",
                "updated",
                "related"
            ]
        );
    }

    #[test]
    fn a_second_identity_claim_conflicts_and_writes_nothing() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        claim(&v.cfg, &note.id, "alice").unwrap();
        let path = note_path(&v, &note.id);
        let before = std::fs::read_to_string(&path).unwrap();

        let err = claim(&v.cfg, &note.id, "bob").unwrap_err();
        assert_eq!(err.code(), 4);
        assert_eq!(
            err.to_string(),
            format!("note {} already claimed by alice", note.id)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_same_identity_reclaim_is_a_byte_identical_noop() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        let first = claim(&v.cfg, &note.id, "alice").unwrap();
        let path = note_path(&v, &note.id);
        let before = std::fs::read_to_string(&path).unwrap();

        let again = claim(&v.cfg, &note.id, "alice").unwrap();
        assert_eq!(again.updated, first.updated, "a no-op never bumps updated");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn release_is_idempotent_and_force_breaks_a_foreign_claim() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        let path = note_path(&v, &note.id);
        let before = std::fs::read_to_string(&path).unwrap();

        // An unclaimed note reports the found state and writes nothing.
        let released = release(&v.cfg, &note.id, "alice", false).unwrap();
        assert_eq!(released.claimed_by, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        claim(&v.cfg, &note.id, "alice").unwrap();
        let held = std::fs::read_to_string(&path).unwrap();
        let err = release(&v.cfg, &note.id, "bob", false).unwrap_err();
        assert_eq!(err.code(), 4);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), held);

        let released = release(&v.cfg, &note.id, "bob", true).unwrap();
        assert_eq!(released.claimed_by, None);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("claimed_by: null\n"));
        // The owner survives a release as well.
        assert_eq!(released.owner.as_deref(), Some("test-agent"));
    }

    #[test]
    fn release_reports_the_holder_it_cleared() {
        let v = vault();
        let note = create(&v.cfg, "T", NewNote::default()).unwrap();
        claim(&v.cfg, &note.id, "alice").unwrap();
        // The holder may release their own claim without --force.
        let released = release(&v.cfg, &note.id, "alice", false).unwrap();
        assert_eq!(released.claimed_by, None);

        // Releasing again is still an idempotent no-op.
        let path = note_path(&v, &note.id);
        let before = std::fs::read_to_string(&path).unwrap();
        release(&v.cfg, &note.id, "alice", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn claim_and_release_resolve_a_slug_and_report_not_found() {
        let v = vault();
        let note = create(&v.cfg, "Team Sol", NewNote::default()).unwrap();
        claim(&v.cfg, "team-sol", "alice").unwrap();
        assert_eq!(
            get(&v.cfg, &note.id).unwrap().item.claimed_by.as_deref(),
            Some("alice")
        );
        release(&v.cfg, "team-sol", "alice", false).unwrap();

        assert_eq!(claim(&v.cfg, "n-NOPE", "alice").unwrap_err().code(), 3);
        assert_eq!(
            release(&v.cfg, "n-NOPE", "alice", false)
                .unwrap_err()
                .code(),
            3
        );
    }
}
