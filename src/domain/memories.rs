//! Memory verbs, recall composition and expiry.
//!
//! A memory is note-shaped: the same base frontmatter, the same lock, atomic-write and stash
//! mechanics, the same `related`/wikilink derivation. The layout is **flat**
//! (`<memories>/m-XXXX.md`) and reads walk recursively, so **no memory verb ever moves a
//! file** — `scope` is a frontmatter filter, never a folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::domain::select::{matches_filters, select, FromMeta};
use crate::domain::{
    apply_tag_spec, effective_owner, resolve_wikilinks, validate_owner, AppendOpts, Filter,
};
use crate::error::{MeshError, Result};
use crate::fm::{read_doc, read_meta_only, write_doc, Doc, Meta, Row, Value, View};
use crate::ids::generate_id;
use crate::model::common::{meta_str, meta_strings, optional_str, ts_value};
use crate::model::memory::{
    Memory, MemorySummary, DEFAULT_IMPORTANCE, DEFAULT_KIND, DEFAULT_SCOPE, MAX_IMPORTANCE,
    MEMORY_FIELDS, MEMORY_ID_PREFIX, MEMORY_KINDS, MEMORY_SCOPES, MEMORY_TYPE, MIN_IMPORTANCE,
};
use crate::search::{self, Engine, Hit, SearchFilter};
use crate::spaces::Space;
use crate::storage::lock::{create_lock, entity_lock, hold};
use crate::storage::{iter_md, realpath, safe_resolve};
use crate::text::{append_to_end, append_under_section, edit_distance, format_block, slugify};
use crate::timefmt::{iso_z, now_utc, parse_since};

/// How many near-miss ids a not-found error carries.
const MAX_CANDIDATES: usize = 5;

/// The `--expires none` token that clears a soft TTL on update.
pub const EXPIRES_NONE: &str = "none";

/// The importance weight's intercept: `0.6 + 0.1 * importance` spans 0.7 .. 1.1.
const IMPORTANCE_INTERCEPT: f64 = 0.6;
/// The importance weight's slope.
const IMPORTANCE_SLOPE: f64 = 0.1;
/// The floor the recency term decays towards: `0.35 + 0.65 * recency`.
const RECENCY_FLOOR: f64 = 0.35;
/// The share of the score the recency term can move.
const RECENCY_SPAN: f64 = 0.65;
/// The half-life of the recency term, in days.
const HALF_LIFE_DAYS: f64 = 90.0;

/// What `memory new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewMemory {
    pub kind: String,
    pub scope: String,
    pub importance: Option<i64>,
    pub source: Option<String>,
    pub expires: Option<DateTime<Utc>>,
    pub supersedes: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
}

/// What `memory update` was asked to change. `expires: Some(None)` clears it.
#[derive(Clone, Debug, Default)]
pub struct UpdateMemory {
    pub tags: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub scope: Option<String>,
    pub importance: Option<i64>,
    pub source: Option<String>,
    pub expires: Option<Option<DateTime<Utc>>>,
    pub owner: Option<String>,
}

/// The `memory list` switches that are not part of the shared `Filter`.
#[derive(Clone, Debug, Default)]
pub struct ListMemoryOpts {
    pub kind: Option<String>,
    pub scope: Option<String>,
    pub min_importance: Option<i64>,
    pub include_expired: bool,
    pub include_superseded: bool,
}

/// The `memory recall` switches.
#[derive(Clone, Debug, Default)]
pub struct RecallOpts {
    pub limit: i64,
    pub threshold: Option<f64>,
    pub decay: bool,
    pub include_expired: bool,
    pub min_importance: Option<i64>,
    pub meta_only: bool,
    pub full: bool,
}

// ---------------------------------------------------------------------------------------
// paths and resolution
// ---------------------------------------------------------------------------------------

fn stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|s| s.to_str())
}

fn is_mesh_stem(path: &Path) -> bool {
    stem(path).is_some_and(|s| s.starts_with(MEMORY_ID_PREFIX))
}

/// Every Markdown file in the memories space, sorted. The walk is **recursive**: the layout
/// mesh writes is flat, but a human who files memories into subfolders keeps working.
fn all_paths(cfg: &Config) -> Vec<PathBuf> {
    let Ok(root) = cfg.root(Space::Memories) else {
        return Vec::new();
    };
    iter_md(root, true, cfg.spaces.exclusions_for(Space::Memories)).collect()
}

/// The files a mesh verb may address. Membership is by filename stem, not by content, so a
/// memory with unparseable frontmatter still resolves — and is then reported not-found.
fn mesh_paths(cfg: &Config) -> Vec<PathBuf> {
    let mut out = all_paths(cfg);
    out.retain(|p| is_mesh_stem(p));
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

fn memory_not_found(target: &str) -> MeshError {
    MeshError::MemoryNotFound(target.to_string())
}

/// Resolve an `m-` id or a title slug to the memory's path.
///
/// An exact stem match wins; otherwise the slugified title is compared against every mesh
/// memory. Several matches are an ambiguous slug (exit 2, ids sorted); none is not-found
/// (exit 3) carrying the near-miss candidates.
pub fn resolve(cfg: &Config, target: &str) -> Result<PathBuf> {
    // A disabled space is exit 2, never "not found": the address is unanswerable, not absent.
    cfg.root(Space::Memories)?;
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
            None => Err(memory_not_found(target)),
        },
        0 => Err(memory_not_found(target).with_candidates(candidates(&paths, target))),
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

/// Resolve to the memory id, which is what names the lock.
fn resolve_id(cfg: &Config, target: &str) -> Result<String> {
    let path = resolve(cfg, target)?;
    stem(&path)
        .map(str::to_string)
        .ok_or_else(|| memory_not_found(target))
}

// ---------------------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------------------

fn validate_kind(value: &str) -> Result<()> {
    if MEMORY_KINDS.contains(&value) {
        return Ok(());
    }
    Err(MeshError::validation(format!(
        "invalid kind: '{value}' (use {})",
        MEMORY_KINDS.join(", ")
    )))
}

fn validate_scope(value: &str) -> Result<()> {
    if MEMORY_SCOPES.contains(&value) {
        return Ok(());
    }
    Err(MeshError::validation(format!(
        "invalid scope: '{value}' (use {})",
        MEMORY_SCOPES.join(", ")
    )))
}

fn validate_importance(value: i64) -> Result<()> {
    if (MIN_IMPORTANCE..=MAX_IMPORTANCE).contains(&value) {
        return Ok(());
    }
    Err(MeshError::validation(format!(
        "invalid importance: '{value}' (use {MIN_IMPORTANCE}..{MAX_IMPORTANCE})"
    )))
}

/// The `--expires` grammar: the `--since` duration as an offset **forward** from now, or an
/// absolute ISO datetime. `--since` reads the same text backwards; that is the only
/// difference between the two flags.
pub fn parse_expires(value: &str) -> Result<DateTime<Utc>> {
    let text = value.trim();
    if let Some(delta) = forward_duration(text) {
        return Ok(now_utc() + delta);
    }
    parse_since(text)
}

/// `^(\d+)([dhw])$` read forwards. Anything else falls through to the absolute ISO branch.
fn forward_duration(text: &str) -> Option<chrono::Duration> {
    let unit = text.chars().next_back()?;
    let digits: String = text
        .chars()
        .take(text.chars().count().saturating_sub(1))
        .collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    match unit {
        'd' => chrono::Duration::try_days(n),
        'h' => chrono::Duration::try_hours(n),
        'w' => chrono::Duration::try_weeks(n),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------------------

/// Record a memory. Id allocation and the write both happen under the create lock.
///
/// `o.supersedes` is honoured and any failure to stamp the old memory is **swallowed** —
/// call [`create_with_warnings`] when the caller has a surface to report it on.
pub fn create(cfg: &Config, title: &str, o: NewMemory) -> Result<Memory> {
    create_with_warnings(cfg, title, o).map(|(memory, _)| memory)
}

/// [`create`], plus the advisory lines a failed supersession produced.
pub fn create_with_warnings(
    cfg: &Config,
    title: &str,
    o: NewMemory,
) -> Result<(Memory, Vec<String>)> {
    let kind = if o.kind.is_empty() {
        DEFAULT_KIND.to_string()
    } else {
        o.kind.clone()
    };
    validate_kind(&kind)?;
    let scope = if o.scope.is_empty() {
        DEFAULT_SCOPE.to_string()
    } else {
        o.scope.clone()
    };
    validate_scope(&scope)?;
    let importance = o.importance.unwrap_or(DEFAULT_IMPORTANCE);
    validate_importance(importance)?;
    validate_owner(cfg, o.owner.as_deref())?;
    let root = cfg.root(Space::Memories)?.to_path_buf();

    let memory = {
        let _guard = hold(&create_lock(&root))?;
        let now = now_utc();
        let taken: Vec<String> = mesh_paths(cfg)
            .iter()
            .filter_map(|p| stem(p))
            .map(str::to_string)
            .collect();
        let id = generate_id(MEMORY_ID_PREFIX, &iso_z(&now), title, &|candidate| {
            taken.iter().any(|t| t == candidate)
        });

        let mut meta = Meta::new();
        meta.insert("id".to_string(), Value::str(id.as_str()));
        meta.insert("type".to_string(), Value::str(MEMORY_TYPE));
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
        meta.insert("kind".to_string(), Value::str(kind.as_str()));
        meta.insert("scope".to_string(), Value::str(scope.as_str()));
        meta.insert("importance".to_string(), Value::Int(importance));
        meta.insert("source".to_string(), optional_str(o.source.as_deref()));
        meta.insert(
            "expires".to_string(),
            match &o.expires {
                Some(at) => ts_value(at),
                None => Value::Null,
            },
        );
        meta.insert("superseded_by".to_string(), Value::Null);

        let path = safe_resolve(&cfg.spaces, &root.join(format!("{id}.md")))?;
        let doc = Doc::new(meta, o.body.clone());
        write_doc(&cfg.spaces, &path, &doc)?;
        Memory::from_meta(&doc.meta).ok_or_else(|| memory_not_found(&id))?
    };

    // The supersession is a second single-entity write under the *old* memory's own lock, so
    // it happens only once the create lock is released.
    let mut warnings: Vec<String> = Vec::new();
    if let Some(old) = o
        .supersedes
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        if let Err(e) = supersede(cfg, old, &memory.id) {
            warnings.push(format!("memory new: could not supersede {old} ({e})"));
        }
    }
    Ok((memory, warnings))
}

/// Stamp `superseded_by` onto an existing memory, under that memory's own lock.
pub fn supersede(cfg: &Config, target: &str, by: &str) -> Result<Memory> {
    let by = by.to_string();
    amend(cfg, target, move |_, doc| {
        doc.meta
            .insert("superseded_by".to_string(), Value::str(by.as_str()));
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------
// amend (append / update / supersede)
// ---------------------------------------------------------------------------------------

/// The shared amend body: resolve, lock, re-resolve inside the lock, read, mutate, recompute
/// `related`, bump `updated` and write — skipping the write entirely when nothing changed.
fn amend(
    cfg: &Config,
    target: &str,
    mutate: impl FnOnce(&Config, &mut Doc) -> Result<()>,
) -> Result<Memory> {
    let id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Memories)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, &id))?;
    // Resolve again inside the lock (the TOCTOU rule).
    let path = resolve(cfg, &id)?;
    let Some(mut doc) = read_doc(&path) else {
        return Err(memory_not_found(target));
    };
    let before = doc.clone();
    mutate(cfg, &mut doc)?;
    doc.meta.insert(
        "related".to_string(),
        Value::strings(resolve_wikilinks(cfg, &doc.body)),
    );
    if doc.meta == before.meta && doc.body == before.body {
        // An idempotent no-op never rewrites the file and never bumps `updated`.
        return Memory::from_meta(&doc.meta).ok_or_else(|| memory_not_found(target));
    }
    doc.meta.insert("updated".to_string(), ts_value(&now_utc()));
    let memory = Memory::from_meta(&doc.meta).ok_or_else(|| memory_not_found(target))?;
    write_doc(&cfg.spaces, &path, &doc)?;
    Ok(memory)
}

/// Append a block to a memory's body, optionally under `## {section}` and timestamped.
pub fn append(cfg: &Config, target: &str, text: &str, o: AppendOpts) -> Result<Memory> {
    let actor = o.actor.clone().or_else(|| cfg.agent().map(str::to_string));
    let block = format_block(text, o.timestamp, actor.as_deref());
    amend(cfg, target, move |_, doc| {
        doc.body = match o.section.as_deref() {
            Some(section) => append_under_section(&doc.body, &block, section),
            None => append_to_end(&doc.body, &block),
        };
        Ok(())
    })
}

/// Update a memory's fields. Every enum value is validated **before** the lock, so a bad
/// `--kind` never touches the file.
pub fn update(cfg: &Config, target: &str, o: UpdateMemory) -> Result<Memory> {
    if let Some(kind) = &o.kind {
        validate_kind(kind)?;
    }
    if let Some(scope) = &o.scope {
        validate_scope(scope)?;
    }
    if let Some(importance) = o.importance {
        validate_importance(importance)?;
    }
    if o.owner.is_some() {
        validate_owner(cfg, o.owner.as_deref())?;
    }
    amend(cfg, target, move |_, doc| {
        if let Some(spec) = &o.tags {
            let next = apply_tag_spec(&meta_strings(&doc.meta, "tags"), spec)?;
            doc.meta.insert("tags".to_string(), Value::strings(next));
        }
        if let Some(title) = &o.title {
            doc.meta
                .insert("title".to_string(), Value::str(title.as_str()));
        }
        if let Some(kind) = &o.kind {
            doc.meta
                .insert("kind".to_string(), Value::str(kind.as_str()));
        }
        if let Some(scope) = &o.scope {
            doc.meta
                .insert("scope".to_string(), Value::str(scope.as_str()));
        }
        if let Some(importance) = o.importance {
            doc.meta
                .insert("importance".to_string(), Value::Int(importance));
        }
        if let Some(source) = &o.source {
            doc.meta
                .insert("source".to_string(), Value::str(source.as_str()));
        }
        if let Some(expires) = &o.expires {
            doc.meta.insert(
                "expires".to_string(),
                match expires {
                    Some(at) => ts_value(at),
                    None => Value::Null,
                },
            );
        }
        if let Some(owner) = &o.owner {
            doc.meta
                .insert("owner".to_string(), Value::str(owner.as_str()));
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------------------

/// Read one memory: frontmatter, body and path.
pub fn get(cfg: &Config, target: &str) -> Result<View<Memory>> {
    let path = resolve(cfg, target)?;
    let Some(doc) = read_doc(&path) else {
        return Err(memory_not_found(target));
    };
    let item = Memory::from_meta(&doc.meta).ok_or_else(|| memory_not_found(target))?;
    Ok(View {
        item,
        body: doc.body,
        path,
    })
}

/// Every readable `(path, frontmatter)` pair in the memories space — the scan every listing,
/// lens and index pass shares. Unreadable and unparseable files are skipped silently.
pub fn rows(cfg: &Config) -> Vec<Row> {
    all_paths(cfg)
        .into_iter()
        .filter_map(|path| read_meta_only(&path).map(|meta| Row { path, meta }))
        .collect()
}

/// Every valid memory in the space, filtered and sorted but never limited.
fn all_views(cfg: &Config, f: &Filter) -> Vec<View<Memory>> {
    let base = Filter {
        limit: None,
        ..f.clone()
    };
    select(rows(cfg), &base)
}

/// `>= 0` slices, negative and `None` are unbounded, `0` yields `[]`.
fn apply_limit<T>(views: &mut Vec<T>, limit: Option<i64>) {
    if let Some(n) = limit {
        if n >= 0 {
            views.truncate(usize::try_from(n).unwrap_or(0));
        }
    }
}

/// List memories. Expired and superseded rows are excluded by default, and a `private`
/// memory owned by somebody else is never listed (a courtesy filter, never authorisation).
pub fn list(cfg: &Config, f: &Filter, o: &ListMemoryOpts) -> Result<Vec<View<Memory>>> {
    cfg.root(Space::Memories)?;
    // `kind` and `scope` match the raw frontmatter value, exactly like `--type` and
    // `--status` elsewhere, so `memory list --kind fact` and `memory recall --kind fact`
    // cannot drift.
    let filter = f
        .clone()
        .with_extra("kind", o.kind.as_deref())
        .with_extra("scope", o.scope.as_deref());
    let now = now_utc();
    let me = f.me.clone();
    let mut views = all_views(cfg, &filter);
    views.retain(|v| {
        let m = &v.item;
        o.min_importance
            .is_none_or(|n| m.effective_importance() >= n)
            && (o.include_expired || !m.is_expired(now))
            && (o.include_superseded || !m.is_superseded())
            && m.is_visible_to(me.as_deref())
    });
    apply_limit(&mut views, f.limit);
    Ok(views)
}

/// The `status` payload's memories block: valid memories, how many have expired, how many
/// were superseded. Scope is not applied — this is the operator's own count.
pub fn summary(cfg: &Config) -> MemorySummary {
    let now = now_utc();
    let mut out = MemorySummary::default();
    for view in all_views(cfg, &Filter::unbounded()) {
        out.total += 1;
        if view.item.is_expired(now) {
            out.expired += 1;
        }
        if view.item.is_superseded() {
            out.superseded += 1;
        }
    }
    out
}

/// The memories `session-start` offers: live, un-superseded, visible to `me`, ranked
/// `importance desc, updated desc` and capped.
pub fn session_picks(cfg: &Config, me: Option<&str>, cap: usize) -> Vec<View<Memory>> {
    let now = now_utc();
    let mut views = all_views(cfg, &Filter::unbounded());
    views
        .retain(|v| !v.item.is_expired(now) && !v.item.is_superseded() && v.item.is_visible_to(me));
    // Stable composition: the weakest key first, the strongest last.
    views.sort_by(|a, b| a.path.to_string_lossy().cmp(&b.path.to_string_lossy()));
    views.sort_by(|a, b| b.item.updated.cmp(&a.item.updated));
    views.sort_by(|a, b| {
        b.item
            .effective_importance()
            .cmp(&a.item.effective_importance())
    });
    views.truncate(cap);
    views
}

/// The id of the first memory whose slugified title matches `title`, if any.
///
/// Slug-normalised and memories-only, so a memory never collides with a note or a task of
/// the same name. Advisory: the caller still creates the memory.
pub fn find_duplicate_title(cfg: &Config, title: &str) -> Option<String> {
    let want = slugify(title);
    rows(cfg).into_iter().find_map(|row| {
        let id = meta_str(&row.meta, "id")?;
        if !id.starts_with(MEMORY_ID_PREFIX) {
            return None;
        }
        if slugify(meta_str(&row.meta, "title")?) == want {
            Some(id.to_string())
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------------------
// recall
// ---------------------------------------------------------------------------------------

/// The recall ranking (final.md §5.5):
///
/// ```text
/// final = match_score * (0.6 + 0.1 * importance) * (0.35 + 0.65 * 0.5^(age_days / 90))
/// ```
///
/// `decay = false` drops the recency term (it becomes 1.0) for audits. Decay is **ranking,
/// never deletion**.
pub fn recall_score(match_score: f64, importance: i64, age_days: f64, decay: bool) -> f64 {
    let weight = IMPORTANCE_INTERCEPT + IMPORTANCE_SLOPE * importance as f64;
    let recency = if decay {
        0.5_f64.powf(age_days / HALF_LIFE_DAYS)
    } else {
        1.0
    };
    match_score * weight * (RECENCY_FLOOR + RECENCY_SPAN * recency)
}

/// The search filter a recall runs: the memories space alone, unbounded, unthresholded.
///
/// The memory predicates and `--threshold` are applied to the **re-ranked** score afterwards,
/// so the ranker sees every candidate and one document's score never depends on which filters
/// another caller passed. `quiet` is set: the `search:` degradation notices name a different
/// verb and recall applies its own ranking on top.
fn recall_search_filter() -> SearchFilter {
    SearchFilter {
        spaces: vec![Space::Memories],
        limit: -1,
        threshold: None,
        engine: Engine::Auto,
        quiet: true,
        ..SearchFilter::default()
    }
}

/// A memory-shaped index of the space, keyed by resolved path.
fn view_index(cfg: &Config) -> HashMap<PathBuf, Memory> {
    let mut out: HashMap<PathBuf, Memory> = HashMap::new();
    for row in rows(cfg) {
        if let Some(memory) = Memory::from_meta(&row.meta) {
            out.insert(realpath(&row.path), memory);
        }
    }
    out
}

/// Recall: a ranked search restricted to the memories space, re-weighted by `importance` and
/// recency. Emits the standard hit array so one parser serves `search` and `recall` alike.
pub fn recall(cfg: &Config, query: &str, f: &Filter, o: &RecallOpts) -> Result<Vec<Hit>> {
    cfg.root(Space::Memories)?;
    let (hits, _mode) = search::query(cfg, query, &recall_search_filter())?;
    let index = view_index(cfg);
    let now = now_utc();
    let me = f.me.clone();

    let mut scored: Vec<(f64, Hit)> = Vec::new();
    for mut hit in hits {
        let Some(memory) = index.get(&realpath(&hit.path)) else {
            continue;
        };
        if !matches_filters(&memory.meta, f) {
            continue;
        }
        if o.min_importance
            .is_some_and(|n| memory.effective_importance() < n)
        {
            continue;
        }
        if !o.include_expired && memory.is_expired(now) {
            continue;
        }
        // A superseded memory never comes back from recall; `memory list
        // --include-superseded` is the audit path.
        if memory.is_superseded() || !memory.is_visible_to(me.as_deref()) {
            continue;
        }
        let final_score = recall_score(
            hit.score,
            memory.effective_importance(),
            memory.age_days(now),
            o.decay,
        );
        if o.threshold.is_some_and(|t| final_score < t) {
            continue;
        }
        hit.score = final_score;
        scored.push((final_score, hit));
    }

    // Stable composition: path ascending, then `updated` descending, then the final score.
    scored.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    scored.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut out: Vec<Hit> = scored.into_iter().map(|(_, hit)| hit).collect();
    apply_limit(&mut out, Some(o.limit));
    if o.full && !o.meta_only {
        search::fill_full_bodies(&mut out);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// forget
// ---------------------------------------------------------------------------------------

/// Hard-delete a memory. Removing a file with corrupt frontmatter is the repair path, so
/// this is the one verb that never validates.
pub fn forget(cfg: &Config, target: &str) -> Result<String> {
    let id = resolve_id(cfg, target)?;
    let root = cfg.root(Space::Memories)?.to_path_buf();
    let _guard = hold(&entity_lock(&root, &id))?;
    let path = resolve(cfg, &id)?;
    std::fs::remove_file(&path)?;
    Ok(id)
}

/// The ids of every expired memory, in listing order. Scope and supersession do not apply:
/// `forget --expired` is the operator's own maintenance sweep.
pub fn expired_ids(cfg: &Config) -> Result<Vec<String>> {
    cfg.root(Space::Memories)?;
    let now = now_utc();
    Ok(all_views(cfg, &Filter::unbounded())
        .into_iter()
        .filter(|v| v.item.is_expired(now))
        .map(|v| v.item.id)
        .collect())
}

/// Hard-delete every expired memory. Returns the ids actually removed, in listing order.
pub fn forget_expired(cfg: &Config) -> Result<Vec<String>> {
    let mut removed: Vec<String> = Vec::new();
    for id in expired_ids(cfg)? {
        forget(cfg, &id)?;
        removed.push(id);
    }
    Ok(removed)
}

/// The `MEMORY_FIELDS` order — the JSON and on-disk key order for every memory payload.
pub fn field_order() -> &'static [&'static str] {
    MEMORY_FIELDS.fields()
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
    use crate::domain::SortKey;

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

    fn memory_file(id: &str, title: &str, extra: &str) -> String {
        format!(
            "---\nid: {id}\ntype: memory\ntitle: {title}\ntags: []\nowner: null\n\
             created: 2026-01-02T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
             kind: fact\nscope: shared\nimportance: 3\nsource: null\n{extra}---\n\nbody\n"
        )
    }

    fn new_memory() -> NewMemory {
        NewMemory {
            kind: DEFAULT_KIND.to_string(),
            scope: DEFAULT_SCOPE.to_string(),
            ..NewMemory::default()
        }
    }

    #[test]
    fn create_writes_a_flat_file_with_the_declared_key_order() {
        let v = vault();
        let m = create(&v.cfg, "Alpha", new_memory()).unwrap();
        assert!(m.id.starts_with("m-"));
        let path = v.cfg.vault().join(format!("memories/{}.md", m.id));
        assert!(path.is_file(), "{}", path.display());
        let doc = read_doc(&path).unwrap();
        let keys: Vec<&str> = doc.meta.keys().map(String::as_str).collect();
        assert_eq!(keys, MEMORY_FIELDS.fields());
        assert_eq!(m.kind, "fact");
        assert_eq!(m.scope, "shared");
        assert_eq!(m.importance, Some(3));
        assert_eq!(m.superseded_by, None);
    }

    #[test]
    fn create_rejects_bad_enums_before_writing() {
        let v = vault();
        for (o, message) in [
            (
                NewMemory {
                    kind: "hunch".into(),
                    ..new_memory()
                },
                "invalid kind: 'hunch' (use fact, preference, procedure, insight, episode)",
            ),
            (
                NewMemory {
                    scope: "team".into(),
                    ..new_memory()
                },
                "invalid scope: 'team' (use shared, private)",
            ),
            (
                NewMemory {
                    importance: Some(9),
                    ..new_memory()
                },
                "invalid importance: '9' (use 1..5)",
            ),
        ] {
            let err = create(&v.cfg, "T", o).unwrap_err();
            assert_eq!(err.code(), 2);
            assert_eq!(err.to_string(), message);
        }
        assert!(rows(&v.cfg).is_empty());
    }

    #[test]
    fn create_derives_related_from_wikilinks() {
        let v = vault();
        write(
            &v.cfg,
            "notes/n-AAAA.md",
            "---\nid: n-AAAA\ntype: note\ntitle: Alpha\ntags: []\nowner: null\n\
             created: 2026-01-02\nupdated: 2026-01-02\nrelated: []\n---\n\nx\n",
        );
        let m = create(
            &v.cfg,
            "Linker",
            NewMemory {
                body: "see [[Alpha]] and [[m-9999]]".into(),
                ..new_memory()
            },
        )
        .unwrap();
        assert_eq!(m.related, ["n-AAAA", "m-9999"]);
    }

    #[test]
    fn supersedes_stamps_the_old_memory_and_warns_when_it_cannot() {
        let v = vault();
        let old = create(&v.cfg, "Old", new_memory()).unwrap();
        let (new, warnings) = create_with_warnings(
            &v.cfg,
            "New",
            NewMemory {
                supersedes: Some(old.id.clone()),
                ..new_memory()
            },
        )
        .unwrap();
        assert!(warnings.is_empty());
        let stamped = get(&v.cfg, &old.id).unwrap().item;
        assert_eq!(stamped.superseded_by.as_deref(), Some(new.id.as_str()));
        assert!(stamped.is_superseded());

        let (_, warnings) = create_with_warnings(
            &v.cfg,
            "Third",
            NewMemory {
                supersedes: Some("m-GHOST".into()),
                ..new_memory()
            },
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0],
            "memory new: could not supersede m-GHOST (memory not found: m-GHOST)"
        );
    }

    #[test]
    fn resolution_accepts_an_id_or_a_slug_and_reports_ambiguity() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-AAAA.md",
            &memory_file("m-AAAA", "My Title", ""),
        );
        assert_eq!(resolve_id(&v.cfg, "m-AAAA").unwrap(), "m-AAAA");
        assert_eq!(resolve_id(&v.cfg, "My Title").unwrap(), "m-AAAA");
        assert_eq!(resolve_id(&v.cfg, "my--title").unwrap(), "m-AAAA");
        write(
            &v.cfg,
            "memories/m-BBBB.md",
            &memory_file("m-BBBB", "My Title", ""),
        );
        let err = resolve(&v.cfg, "my-title").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "ambiguous slug 'my-title': m-AAAA, m-BBBB");
    }

    #[test]
    fn a_miss_is_exit_three_with_candidates() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-AAAA.md",
            &memory_file("m-AAAA", "Japan Visa", ""),
        );
        let err = resolve(&v.cfg, "japan-visas").unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "memory not found: japan-visas");
        assert_eq!(err.candidates(), ["m-AAAA"]);
    }

    #[test]
    fn a_subfolder_is_read_but_never_moved() {
        let v = vault();
        write(
            &v.cfg,
            "memories/personal/m-AAAA.md",
            &memory_file("m-AAAA", "Filed", ""),
        );
        assert_eq!(get(&v.cfg, "m-AAAA").unwrap().item.title, "Filed");
        update(
            &v.cfg,
            "m-AAAA",
            UpdateMemory {
                title: Some("Renamed".into()),
                ..UpdateMemory::default()
            },
        )
        .unwrap();
        assert!(v.cfg.vault().join("memories/personal/m-AAAA.md").is_file());
        assert!(!v.cfg.vault().join("memories/m-AAAA.md").exists());
    }

    #[test]
    fn a_corrupt_memory_is_not_found_but_still_forgettable() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-BAD.md",
            "---\nid: m-BAD\ntitle: [unclosed\n---\n\nbroken\n",
        );
        assert_eq!(get(&v.cfg, "m-BAD").unwrap_err().code(), 3);
        assert_eq!(
            append(&v.cfg, "m-BAD", "x", AppendOpts::default())
                .unwrap_err()
                .code(),
            3
        );
        assert_eq!(forget(&v.cfg, "m-BAD").unwrap(), "m-BAD");
        assert!(!v.cfg.vault().join("memories/m-BAD.md").exists());
    }

    #[test]
    fn append_bumps_updated_and_keeps_created() {
        let v = vault();
        let m = create(&v.cfg, "Alpha", new_memory()).unwrap();
        let after = append(&v.cfg, &m.id, "more", AppendOpts::default()).unwrap();
        assert_eq!(after.created, m.created);
        assert!(after.updated >= m.updated);
        let body = read_doc(&resolve(&v.cfg, &m.id).unwrap()).unwrap().body;
        assert_eq!(body, "more");
    }

    #[test]
    fn append_under_a_section_creates_it_when_absent() {
        let v = vault();
        let m = create(
            &v.cfg,
            "Alpha",
            NewMemory {
                body: "Intro.\n\n## A\n\nitem1".into(),
                ..new_memory()
            },
        )
        .unwrap();
        append(
            &v.cfg,
            &m.id,
            "NEW",
            AppendOpts {
                section: Some("A".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        let path = resolve(&v.cfg, &m.id).unwrap();
        assert_eq!(
            read_doc(&path).unwrap().body,
            "Intro.\n\n## A\n\nitem1\n\nNEW"
        );
    }

    #[test]
    fn update_sets_every_field_and_clears_the_expiry() {
        let v = vault();
        let m = create(&v.cfg, "Alpha", new_memory()).unwrap();
        let at = parse_expires("7d").unwrap();
        let updated = update(
            &v.cfg,
            &m.id,
            UpdateMemory {
                title: Some("Beta".into()),
                kind: Some("insight".into()),
                scope: Some("private".into()),
                importance: Some(5),
                source: Some("chat".into()),
                expires: Some(Some(at)),
                tags: Some("a,b".into()),
                ..UpdateMemory::default()
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Beta");
        assert_eq!(updated.kind, "insight");
        assert_eq!(updated.scope, "private");
        assert_eq!(updated.importance, Some(5));
        assert_eq!(updated.source.as_deref(), Some("chat"));
        assert_eq!(updated.tags, ["a", "b"]);
        assert!(updated.expires.is_some());
        let cleared = update(
            &v.cfg,
            &m.id,
            UpdateMemory {
                expires: Some(None),
                ..UpdateMemory::default()
            },
        )
        .unwrap();
        assert_eq!(cleared.expires, None);
    }

    #[test]
    fn update_rejects_bad_values_before_the_write() {
        let v = vault();
        let m = create(&v.cfg, "Alpha", new_memory()).unwrap();
        let path = resolve(&v.cfg, &m.id).unwrap();
        let before = std::fs::read(&path).unwrap();
        for o in [
            UpdateMemory {
                kind: Some("hunch".into()),
                ..UpdateMemory::default()
            },
            UpdateMemory {
                scope: Some("team".into()),
                ..UpdateMemory::default()
            },
            UpdateMemory {
                importance: Some(0),
                ..UpdateMemory::default()
            },
            UpdateMemory {
                tags: Some("+a,b".into()),
                ..UpdateMemory::default()
            },
        ] {
            assert_eq!(update(&v.cfg, &m.id, o).unwrap_err().code(), 2);
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn an_idempotent_update_never_rewrites_the_file() {
        let v = vault();
        let m = create(
            &v.cfg,
            "Alpha",
            NewMemory {
                tags: vec!["a".into()],
                ..new_memory()
            },
        )
        .unwrap();
        let path = resolve(&v.cfg, &m.id).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let again = update(
            &v.cfg,
            &m.id,
            UpdateMemory {
                tags: Some("a".into()),
                ..UpdateMemory::default()
            },
        )
        .unwrap();
        assert_eq!(again.updated, m.updated);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_owner_roster_is_enforced_on_create_and_reassign() {
        let mut v = vault();
        v.cfg.tasks.collections = vec!["alice".into()];
        let err = create(
            &v.cfg,
            "T",
            NewMemory {
                owner: Some("ghost".into()),
                ..new_memory()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "unknown owner: 'ghost'");
        assert!(rows(&v.cfg).is_empty());

        let m = create(
            &v.cfg,
            "T",
            NewMemory {
                owner: Some("alice".into()),
                ..new_memory()
            },
        )
        .unwrap();
        let err = update(
            &v.cfg,
            &m.id,
            UpdateMemory {
                owner: Some("ghost".into()),
                ..UpdateMemory::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn list_hides_expired_superseded_and_other_agents_private_memories() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-LIVE.md",
            &memory_file("m-LIVE", "Live", "expires: null\nsuperseded_by: null\n"),
        );
        write(
            &v.cfg,
            "memories/m-GONE.md",
            &memory_file(
                "m-GONE",
                "Gone",
                "expires: 2000-01-01T00:00:00Z\nsuperseded_by: null\n",
            ),
        );
        write(
            &v.cfg,
            "memories/m-OLD.md",
            &memory_file("m-OLD", "Old", "expires: null\nsuperseded_by: m-LIVE\n"),
        );
        let private = memory_file("m-PRIV", "Priv", "expires: null\nsuperseded_by: null\n")
            .replace("owner: null", "owner: alice")
            .replace("scope: shared", "scope: private");
        write(&v.cfg, "memories/m-PRIV.md", &private);

        let ids = |f: &Filter, o: &ListMemoryOpts| -> Vec<String> {
            list(&v.cfg, f, o)
                .unwrap()
                .into_iter()
                .map(|v| v.item.id)
                .collect()
        };
        let bare = Filter::unbounded();
        assert_eq!(ids(&bare, &ListMemoryOpts::default()), ["m-LIVE"]);
        assert_eq!(
            ids(
                &bare,
                &ListMemoryOpts {
                    include_expired: true,
                    ..ListMemoryOpts::default()
                }
            ),
            // The three share an `updated`, so the tie breaks on path ascending.
            ["m-GONE", "m-LIVE"]
        );
        assert_eq!(
            ids(
                &bare,
                &ListMemoryOpts {
                    include_superseded: true,
                    ..ListMemoryOpts::default()
                }
            ),
            ["m-LIVE", "m-OLD"]
        );
        let as_alice = Filter {
            me: Some("alice".into()),
            ..Filter::unbounded()
        };
        assert_eq!(
            ids(&as_alice, &ListMemoryOpts::default()),
            ["m-LIVE", "m-PRIV"]
        );
    }

    #[test]
    fn list_filters_kind_scope_and_min_importance() {
        let v = vault();
        for (id, kind, importance) in [("m-A", "fact", 1), ("m-B", "insight", 5)] {
            let text = memory_file(id, id, "expires: null\nsuperseded_by: null\n")
                .replace("kind: fact", &format!("kind: {kind}"))
                .replace("importance: 3", &format!("importance: {importance}"));
            write(&v.cfg, &format!("memories/{id}.md"), &text);
        }
        let ids = |o: ListMemoryOpts| -> Vec<String> {
            list(&v.cfg, &Filter::unbounded(), &o)
                .unwrap()
                .into_iter()
                .map(|v| v.item.id)
                .collect()
        };
        assert_eq!(
            ids(ListMemoryOpts {
                kind: Some("insight".into()),
                ..ListMemoryOpts::default()
            }),
            ["m-B"]
        );
        assert_eq!(
            ids(ListMemoryOpts {
                scope: Some("shared".into()),
                ..ListMemoryOpts::default()
            })
            .len(),
            2
        );
        assert_eq!(
            ids(ListMemoryOpts {
                min_importance: Some(3),
                ..ListMemoryOpts::default()
            }),
            ["m-B"]
        );
    }

    #[test]
    fn sorting_by_importance_is_descending_with_created_ascending() {
        let v = vault();
        for (id, importance, created) in [
            ("m-A", 5, "2026-03-01T00:00:00Z"),
            ("m-B", 5, "2026-01-01T00:00:00Z"),
            ("m-C", 1, "2026-02-01T00:00:00Z"),
        ] {
            let text = memory_file(id, id, "expires: null\nsuperseded_by: null\n")
                .replace("importance: 3", &format!("importance: {importance}"))
                .replace(
                    "created: 2026-01-02T00:00:00Z",
                    &format!("created: {created}"),
                );
            write(&v.cfg, &format!("memories/{id}.md"), &text);
        }
        let f = Filter {
            sort: SortKey::Importance,
            ..Filter::unbounded()
        };
        let ids: Vec<String> = list(&v.cfg, &f, &ListMemoryOpts::default())
            .unwrap()
            .into_iter()
            .map(|v| v.item.id)
            .collect();
        assert_eq!(ids, ["m-B", "m-A", "m-C"]);
    }

    #[test]
    fn the_summary_counts_total_expired_and_superseded() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-A.md",
            &memory_file(
                "m-A",
                "A",
                "expires: 2000-01-01T00:00:00Z\nsuperseded_by: m-B\n",
            ),
        );
        write(
            &v.cfg,
            "memories/m-B.md",
            &memory_file("m-B", "B", "expires: null\nsuperseded_by: null\n"),
        );
        write(&v.cfg, "memories/loose.md", "just text\n");
        let s = summary(&v.cfg);
        assert_eq!((s.total, s.expired, s.superseded), (2, 1, 1));
    }

    #[test]
    fn session_picks_rank_importance_then_recency_and_cap() {
        let v = vault();
        for (id, importance, updated) in [
            ("m-A", 5, "2026-01-01T00:00:00Z"),
            ("m-B", 5, "2026-05-01T00:00:00Z"),
            ("m-C", 4, "2026-09-01T00:00:00Z"),
        ] {
            let text = memory_file(id, id, "expires: null\nsuperseded_by: null\n")
                .replace("importance: 3", &format!("importance: {importance}"))
                .replace(
                    "updated: 2026-01-02T00:00:00Z",
                    &format!("updated: {updated}"),
                );
            write(&v.cfg, &format!("memories/{id}.md"), &text);
        }
        let picks: Vec<String> = session_picks(&v.cfg, Some("test-agent"), 5)
            .into_iter()
            .map(|v| v.item.id)
            .collect();
        assert_eq!(picks, ["m-B", "m-A", "m-C"]);
        assert_eq!(session_picks(&v.cfg, None, 2).len(), 2);
    }

    #[test]
    fn recall_ranks_importance_and_recency_and_honours_no_decay() {
        let v = vault();
        let stale = iso_z(&(now_utc() - chrono::Duration::days(360)));
        for (id, importance) in [("m-HI", 5), ("m-LO", 1)] {
            let text = memory_file(id, "Widget notes", "expires: null\nsuperseded_by: null\n")
                .replace("importance: 3", &format!("importance: {importance}"))
                .replace(
                    "updated: 2026-01-02T00:00:00Z",
                    &format!("updated: {stale}"),
                )
                .replace("\nbody\n", "\nthe widget is blue\n");
            write(&v.cfg, &format!("memories/{id}.md"), &text);
        }
        let hits = recall(
            &v.cfg,
            "widget",
            &Filter::unbounded(),
            &RecallOpts {
                limit: 10,
                decay: true,
                ..RecallOpts::default()
            },
        )
        .unwrap();
        let ids: Vec<&str> = hits.iter().filter_map(|h| h.id.as_deref()).collect();
        assert_eq!(ids, ["m-HI", "m-LO"]);
        // The recency term is shared, so the ratio is the importance weights alone.
        assert!((hits[0].score / hits[1].score - 1.1 / 0.7).abs() < 1e-9);

        let audited = recall(
            &v.cfg,
            "widget",
            &Filter::unbounded(),
            &RecallOpts {
                limit: 10,
                decay: false,
                ..RecallOpts::default()
            },
        )
        .unwrap();
        assert!(
            audited[0].score > hits[0].score,
            "decay only ever lowers a score"
        );
    }

    #[test]
    fn recall_excludes_expired_superseded_and_private_memories() {
        let v = vault();
        for (id, extra) in [
            ("m-LIVE", "expires: null\nsuperseded_by: null\n"),
            (
                "m-GONE",
                "expires: 2000-01-01T00:00:00Z\nsuperseded_by: null\n",
            ),
            ("m-OLD", "expires: null\nsuperseded_by: m-LIVE\n"),
        ] {
            let text = memory_file(id, id, extra).replace("\nbody\n", "\nwidget\n");
            write(&v.cfg, &format!("memories/{id}.md"), &text);
        }
        let text = memory_file("m-PRIV", "Priv", "expires: null\nsuperseded_by: null\n")
            .replace("owner: null", "owner: alice")
            .replace("scope: shared", "scope: private")
            .replace("\nbody\n", "\nwidget\n");
        write(&v.cfg, "memories/m-PRIV.md", &text);

        let opts = RecallOpts {
            limit: 10,
            decay: true,
            ..RecallOpts::default()
        };
        let ids = |f: &Filter, o: &RecallOpts| -> Vec<String> {
            recall(&v.cfg, "widget", f, o)
                .unwrap()
                .into_iter()
                .filter_map(|h| h.id)
                .collect()
        };
        assert_eq!(ids(&Filter::unbounded(), &opts), ["m-LIVE"]);
        assert_eq!(
            ids(
                &Filter::unbounded(),
                &RecallOpts {
                    include_expired: true,
                    ..opts.clone()
                }
            )
            .len(),
            2
        );
        let as_alice = Filter {
            me: Some("alice".into()),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&as_alice, &opts).len(), 2);
    }

    #[test]
    fn the_recall_threshold_applies_to_the_final_score() {
        let v = vault();
        let text = memory_file("m-A", "Widget", "expires: null\nsuperseded_by: null\n")
            .replace("importance: 3", "importance: 1");
        write(&v.cfg, "memories/m-A.md", &text);
        let base = RecallOpts {
            limit: 10,
            decay: false,
            ..RecallOpts::default()
        };
        let hits = recall(&v.cfg, "widget", &Filter::unbounded(), &base).unwrap();
        assert_eq!(hits.len(), 1);
        let score = hits[0].score;
        assert!(
            score < 1.0,
            "the importance weight lowers a 1.0 title match"
        );
        let filtered = recall(
            &v.cfg,
            "widget",
            &Filter::unbounded(),
            &RecallOpts {
                threshold: Some(score + 0.01),
                ..base.clone()
            },
        )
        .unwrap();
        assert!(filtered.is_empty());
    }

    #[test]
    fn recall_score_matches_the_documented_formula() {
        assert!((recall_score(1.0, 3, 0.0, false) - 0.9).abs() < 1e-12);
        assert!((recall_score(1.0, 1, 0.0, false) - 0.7).abs() < 1e-12);
        assert!((recall_score(1.0, 5, 0.0, false) - 1.1).abs() < 1e-12);
        // A fresh memory's recency term is 1.0, so decay changes nothing at age 0.
        assert!((recall_score(1.0, 3, 0.0, true) - 0.9).abs() < 1e-12);
        // One half-life halves the recency term: 0.35 + 0.65 * 0.5 = 0.675.
        assert!((recall_score(1.0, 3, 90.0, true) - 0.9 * 0.675).abs() < 1e-12);
        assert!((recall_score(0.5, 3, 0.0, false) - 0.45).abs() < 1e-12);
    }

    #[test]
    fn forget_expired_removes_every_expired_memory() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-A.md",
            &memory_file(
                "m-A",
                "A",
                "expires: 2000-01-01T00:00:00Z\nsuperseded_by: null\n",
            ),
        );
        write(
            &v.cfg,
            "memories/m-B.md",
            &memory_file("m-B", "B", "expires: null\nsuperseded_by: null\n"),
        );
        assert_eq!(expired_ids(&v.cfg).unwrap(), ["m-A"]);
        assert_eq!(forget_expired(&v.cfg).unwrap(), ["m-A"]);
        assert!(!v.cfg.vault().join("memories/m-A.md").exists());
        assert!(v.cfg.vault().join("memories/m-B.md").is_file());
        assert!(forget_expired(&v.cfg).unwrap().is_empty());
    }

    #[test]
    fn duplicate_titles_are_slug_normalised_and_memories_only() {
        let v = vault();
        let first = create(&v.cfg, "Japan Visa", new_memory()).unwrap();
        assert_eq!(
            find_duplicate_title(&v.cfg, "  japan   visa!"),
            Some(first.id.clone())
        );
        assert_eq!(find_duplicate_title(&v.cfg, "Something Else"), None);
        write(
            &v.cfg,
            "notes/n-AAAA.md",
            "---\nid: n-AAAA\ntype: note\ntitle: Other\ncreated: 2026-01-02\n\
             updated: 2026-01-02\n---\n\nx\n",
        );
        assert_eq!(find_duplicate_title(&v.cfg, "Other"), None);
    }

    #[test]
    fn the_expires_grammar_reads_durations_forwards() {
        let now = now_utc();
        let week = parse_expires("7d").unwrap();
        assert!((week - now).num_hours() >= 167);
        let hours = parse_expires("12h").unwrap();
        assert!(hours > now);
        let weeks = parse_expires("2w").unwrap();
        assert!((weeks - now).num_days() >= 13);
        let absolute = parse_expires("2030-01-02T03:04:05Z").unwrap();
        assert_eq!(iso_z(&absolute), "2030-01-02T03:04:05Z");
        let err = parse_expires("soon").unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn a_disabled_memories_space_is_a_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Memories, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        assert_eq!(create(&cfg, "T", new_memory()).unwrap_err().code(), 2);
        assert_eq!(
            list(&cfg, &Filter::unbounded(), &ListMemoryOpts::default())
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(
            recall(&cfg, "q", &Filter::unbounded(), &RecallOpts::default())
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(expired_ids(&cfg).unwrap_err().code(), 2);
        assert!(rows(&cfg).is_empty());
        assert_eq!(summary(&cfg).total, 0);
    }

    #[test]
    fn rows_skip_unparseable_files_but_keep_foreign_ones() {
        let v = vault();
        write(&v.cfg, "memories/m-A.md", &memory_file("m-A", "A", ""));
        write(
            &v.cfg,
            "memories/m-BAD.md",
            "---\ntitle: [unclosed\n---\n\nx\n",
        );
        write(&v.cfg, "memories/loose.md", "# Loose\n");
        assert_eq!(rows(&v.cfg).len(), 2, "the unparseable file is skipped");
        assert_eq!(
            list(&v.cfg, &Filter::unbounded(), &ListMemoryOpts::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn unknown_keys_round_trip_through_an_amend() {
        let v = vault();
        write(
            &v.cfg,
            "memories/m-HAND.md",
            "---\nid: m-HAND\ntitle: Hand\ncustom_key: keep me\nextra:\n  nested: v\n\
             created: 2026-01-02\nupdated: 2026-01-02T03:04:05\n---\n\nbody\n",
        );
        append(&v.cfg, "m-HAND", "more", AppendOpts::default()).unwrap();
        let doc = read_doc(&v.cfg.vault().join("memories/m-HAND.md")).unwrap();
        assert_eq!(meta_str(&doc.meta, "custom_key"), Some("keep me"));
        assert!(doc.meta.contains_key("extra"));
        assert!(doc.meta.contains_key("related"));
    }

    #[test]
    fn the_field_order_helper_matches_the_model() {
        assert_eq!(field_order(), MEMORY_FIELDS.fields());
        assert_eq!(EXPIRES_NONE, "none");
    }
}
