//! Note verbs: create, append, update, get, list, delete, plus the resolution seam every
//! other space's lens calls.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::fm::{read_body, read_doc, read_meta_only, write_doc, Doc, Meta, Row, Value, View};
use crate::ids::generate_id;
use crate::model::common::{meta_str, meta_strings, optional_str, ts_value};
use crate::model::note::{ForeignView, Note, NOTE_ID_PREFIX, NOTE_TYPES};
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

/// Every Markdown file in the notes space, sorted, dot components and exclusions skipped.
fn all_paths(cfg: &Config) -> Vec<PathBuf> {
    let Ok(root) = cfg.root(Space::Notes) else {
        return Vec::new();
    };
    iter_md(root, true, cfg.spaces.exclusions_for(Space::Notes)).collect()
}

/// The files a mesh verb may address. Membership is by filename stem, not by content, so a
/// note with unparseable frontmatter still resolves — and is then reported not-found.
fn mesh_paths(cfg: &Config) -> Vec<PathBuf> {
    let mut out = all_paths(cfg);
    out.retain(|p| is_mesh_stem(p));
    out
}

/// Non-mesh Markdown in the notes space: invisible to every mutating verb.
fn foreign_paths(cfg: &Config) -> Vec<PathBuf> {
    let mut out = all_paths(cfg);
    out.retain(|p| !is_mesh_stem(p));
    out
}

/// Up to five nearest ids, by edit distance over both the id and the title slug.
fn candidates(paths: &[PathBuf], target: &str) -> Vec<String> {
    let want = slugify(target);
    let lower = target.to_lowercase();
    let mut scored: Vec<(usize, String)> = Vec::new();
    for path in paths {
        let Some(id) = stem(path) else {
            continue;
        };
        let mut best = edit_distance(&lower, &id.to_lowercase());
        if let Some(title) = read_meta_only(path)
            .as_ref()
            .and_then(|m| meta_str(m, "title").map(str::to_string))
        {
            best = best.min(edit_distance(&want, &slugify(&title)));
        }
        scored.push((best, id.to_string()));
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
/// An exact stem match wins; otherwise the slugified title is compared against every mesh
/// note. Several matches are an ambiguous slug (exit 2, ids sorted); none is not-found
/// (exit 3) carrying the near-miss candidates.
pub fn resolve(cfg: &Config, target: &str) -> Result<PathBuf> {
    let paths = mesh_paths(cfg);
    if let Some(hit) = paths.iter().find(|p| stem(p) == Some(target)) {
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
                .filter_map(|p| stem(p))
                .map(str::to_string)
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

/// Resolve to the note id, which is what names the lock.
fn resolve_id(cfg: &Config, target: &str) -> Result<String> {
    let path = resolve(cfg, target)?;
    stem(&path)
        .map(str::to_string)
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
    let taken: Vec<String> = mesh_paths(cfg)
        .iter()
        .filter_map(|p| stem(p))
        .map(str::to_string)
        .collect();
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
    write_doc(&cfg.spaces, &path, &doc)?;
    Note::from_meta(&doc.meta).ok_or(MeshError::NoteNotFound(id))
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

    let _guard = hold(&entity_lock(&root, &note_id))?;
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
    write_doc(&cfg.spaces, &path, &doc)?;
    Ok(note)
}

/// Update a note's tags, type or title. A type change moves the file inside the lock.
pub fn update(cfg: &Config, target: &str, o: UpdateNote) -> Result<Note> {
    if let Some(new_type) = &o.new_type {
        validate_type(new_type)?;
    }
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();

    let _guard = hold(&entity_lock(&root, &note_id))?;
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
    restamp(cfg, &mut doc);
    let note = Note::from_meta(&doc.meta).ok_or_else(|| note_not_found(target))?;
    write_doc(&cfg.spaces, &path, &doc)?;

    if let Some(new_type) = &o.new_type {
        let Some(name) = path.file_name() else {
            return Ok(note);
        };
        let dest = safe_resolve(&cfg.spaces, &note_folder(cfg, new_type)?.join(name))?;
        if dest != path {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&path, &dest)?;
        }
    }
    Ok(note)
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

/// List mesh notes: id-bearing, schema-valid files only.
///
/// `foreign` cannot be honoured through this return type — a `View<Note>` has no shape for a
/// file with no frontmatter — so `note list --foreign` reads its extra rows from
/// [`foreign_rows`] and concatenates them.
pub fn list(cfg: &Config, f: &Filter, foreign: bool) -> Result<Vec<View<Note>>> {
    let _ = foreign;
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

/// Hard-delete a note. Removing a file with corrupt frontmatter is the repair path, so this
/// is the one verb that never validates.
pub fn delete(cfg: &Config, target: &str) -> Result<String> {
    let note_id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Notes)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, &note_id))?;
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
}
