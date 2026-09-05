// STUB: owned by agent 4 (scratch). Signatures are frozen; the bodies are not.
//! Scratch verbs — name-addressed per-agent working state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::owner::validate_owner;
use crate::domain::select::{matches_filters, sort_views, FromMeta, SortKey};
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::{read_doc, write_doc, Doc, Meta, Value, View};
use crate::model::common::{meta_strings, meta_time, ts_value};
use crate::model::scratch::{Scratch, ScratchSummary, SCRATCH_FIELDS};
use crate::spaces::Space;
use crate::storage::{hold, iter_md};
use crate::text::{append_to_end, append_under_section, format_block, slugify};
use crate::timefmt::now_utc;

/// Slugify `name`; a name that slugifies to empty is `invalid scratch name: '{name}'`.
fn require_name_slug(name: &str) -> Result<String> {
    let slug = slugify(name);
    if slug.is_empty() {
        return Err(MeshError::validation(format!(
            "invalid scratch name: '{name}'"
        )));
    }
    Ok(slug)
}

/// `<scratch-root>/<agent-slug>/<name-slug>.md`.
fn scratch_path(root: &Path, agent_slug: &str, name_slug: &str) -> PathBuf {
    root.join(agent_slug).join(format!("{name_slug}.md"))
}

/// `<scratch-root>/.locks/<agent-slug>/<name-slug>.lock` (final.md §4.1).
fn lock_path(root: &Path, agent_slug: &str, name_slug: &str) -> PathBuf {
    root.join(".locks")
        .join(agent_slug)
        .join(format!("{name_slug}.lock"))
}

/// Build the view straight from the address (authoritative) plus whatever the doc holds.
///
/// Single-entity verbs know `name`/`agent` from the CLI arguments, not from frontmatter, so
/// they never depend on `Scratch::from_meta` succeeding — only a directory scan (`list`) does.
fn scratch_view(name_slug: &str, agent_slug: &str, doc: &Doc) -> Scratch {
    Scratch {
        name: name_slug.to_string(),
        agent: agent_slug.to_string(),
        tags: meta_strings(&doc.meta, "tags"),
        created: meta_time(&doc.meta, "created"),
        updated: meta_time(&doc.meta, "updated"),
        bytes: doc.body.len() as u64,
        meta: doc.meta.clone(),
    }
}

/// Merge the owned keys into `meta` (create defaults, or preserve on amend) and reorder.
fn stamp_meta(existing: Option<&Meta>, name_slug: &str, agent_slug: &str, stamps: &Meta) -> Meta {
    let mut meta = existing.cloned().unwrap_or_default();
    meta.insert("type".to_string(), Value::str("scratch"));
    meta.insert("name".to_string(), Value::str(name_slug));
    meta.insert("agent".to_string(), Value::str(agent_slug));
    if !meta.contains_key("tags") {
        meta.insert("tags".to_string(), Value::List(Vec::new()));
    }
    if !meta.contains_key("created") {
        if let Some(created) = stamps.get("created") {
            meta.insert("created".to_string(), created.clone());
        }
    }
    if let Some(updated) = stamps.get("updated") {
        meta.insert("updated".to_string(), updated.clone());
    }
    SCRATCH_FIELDS.reorder(&meta)
}

pub fn set(cfg: &Config, agent: &str, name: &str, body: &str) -> Result<Scratch> {
    let root = cfg.root(Space::Scratch)?.to_path_buf();
    let name_slug = require_name_slug(name)?;
    let agent_slug = slugify(agent);
    validate_owner(cfg, Some(agent))?;
    let path = scratch_path(&root, &agent_slug, &name_slug);
    let _guard = hold(&lock_path(&root, &agent_slug, &name_slug))?;
    // Resolve again inside the lock (the TOCTOU rule).
    let existing = read_doc(&path);
    if let Some(doc) = &existing {
        if doc.body == body {
            // Idempotent no-op: an identical body leaves the file bytes untouched.
            return Ok(scratch_view(&name_slug, &agent_slug, doc));
        }
    }
    let now = now_utc();
    let mut stamps = Meta::new();
    stamps.insert("created".to_string(), ts_value(&now));
    stamps.insert("updated".to_string(), ts_value(&now));
    let meta = stamp_meta(
        existing.as_ref().map(|d| &d.meta),
        &name_slug,
        &agent_slug,
        &stamps,
    );
    let doc = Doc::new(meta, body.to_string());
    write_doc(&cfg.spaces, &path, &doc)?;
    Ok(scratch_view(&name_slug, &agent_slug, &doc))
}

pub fn append(cfg: &Config, agent: &str, name: &str, text: &str, o: AppendOpts) -> Result<Scratch> {
    let root = cfg.root(Space::Scratch)?.to_path_buf();
    let name_slug = require_name_slug(name)?;
    let agent_slug = slugify(agent);
    validate_owner(cfg, Some(agent))?;
    let path = scratch_path(&root, &agent_slug, &name_slug);
    let _guard = hold(&lock_path(&root, &agent_slug, &name_slug))?;
    let existing = read_doc(&path).ok_or_else(|| MeshError::ScratchNotFound(name.to_string()))?;
    let block = format_block(text, o.timestamp, o.actor.as_deref());
    let new_body = match &o.section {
        Some(section) => append_under_section(&existing.body, &block, section),
        None => append_to_end(&existing.body, &block),
    };
    let now = now_utc();
    let mut meta = existing.meta.clone();
    meta.insert("updated".to_string(), ts_value(&now));
    let meta = SCRATCH_FIELDS.reorder(&meta);
    let doc = Doc::new(meta, new_body);
    write_doc(&cfg.spaces, &path, &doc)?;
    Ok(scratch_view(&name_slug, &agent_slug, &doc))
}

pub fn get(cfg: &Config, agent: &str, name: &str) -> Result<View<Scratch>> {
    let root = cfg.root(Space::Scratch)?.to_path_buf();
    let name_slug = require_name_slug(name)?;
    let agent_slug = slugify(agent);
    validate_owner(cfg, Some(agent))?;
    let path = scratch_path(&root, &agent_slug, &name_slug);
    let doc = read_doc(&path).ok_or_else(|| MeshError::ScratchNotFound(name.to_string()))?;
    let item = scratch_view(&name_slug, &agent_slug, &doc);
    Ok(View {
        item,
        body: doc.body,
        path,
    })
}

pub fn list(
    cfg: &Config,
    agent: Option<&str>,
    all: bool,
    f: &Filter,
) -> Result<Vec<View<Scratch>>> {
    let root = cfg.root(Space::Scratch)?.to_path_buf();
    if let Some(a) = agent {
        validate_owner(cfg, Some(a))?;
    }
    let excl = cfg.spaces.exclusions_for(Space::Scratch);
    let scan_root = if all {
        root.clone()
    } else {
        let agent_slug = agent.map(slugify).ok_or_else(|| {
            MeshError::validation("no agent identity: set [core].agent or pass --owner")
        })?;
        root.join(agent_slug)
    };
    let recursive = all;

    let mut views: Vec<View<Scratch>> = Vec::new();
    for path in iter_md(&scan_root, recursive, excl) {
        let Some(doc) = read_doc(&path) else {
            continue;
        };
        if !matches_filters(&doc.meta, f) {
            continue;
        }
        let Some(mut item) = Scratch::from_meta(&doc.meta) else {
            continue;
        };
        item.bytes = doc.body.len() as u64;
        views.push(View {
            item,
            body: String::new(),
            path,
        });
    }

    // The generic engine's tiebreak is path-ascending; scratch wants name-ascending, so the
    // composition is built by hand: name asc first, then updated desc on top (stable sort).
    views.sort_by(|a, b| a.item.name.to_lowercase().cmp(&b.item.name.to_lowercase()));
    sort_views(&mut views, SortKey::Updated);

    if let Some(n) = f.limit {
        if n >= 0 {
            let n = usize::try_from(n).unwrap_or(0);
            views.truncate(n);
        }
    }
    Ok(views)
}

pub fn clear(cfg: &Config, agent: &str, name: &str) -> Result<String> {
    let root = cfg.root(Space::Scratch)?.to_path_buf();
    let name_slug = require_name_slug(name)?;
    let agent_slug = slugify(agent);
    validate_owner(cfg, Some(agent))?;
    let path = scratch_path(&root, &agent_slug, &name_slug);
    let _guard = hold(&lock_path(&root, &agent_slug, &name_slug))?;
    if !path.is_file() {
        return Err(MeshError::ScratchNotFound(name.to_string()));
    }
    std::fs::remove_file(&path)?;
    Ok(name_slug)
}

pub fn summary(cfg: &Config) -> ScratchSummary {
    let Ok(root) = cfg.root(Space::Scratch) else {
        return ScratchSummary::default();
    };
    let excl = cfg.spaces.exclusions_for(Space::Scratch);
    let mut agents: HashSet<String> = HashSet::new();
    let mut files: u64 = 0;
    for path in iter_md(root, true, excl) {
        files += 1;
        if let Some(agent) = path
            .strip_prefix(root)
            .ok()
            .and_then(|rel| rel.components().next())
            .and_then(|c| c.as_os_str().to_str())
        {
            agents.insert(agent.to_string());
        }
    }
    ScratchSummary {
        files,
        agents: agents.len() as u64,
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

    fn cfg_at(dir: &Path) -> Config {
        config_for(dir)
    }

    #[test]
    fn require_name_slug_rejects_empty_slugs() {
        let err = require_name_slug("---").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "invalid scratch name: '---'");
        assert_eq!(require_name_slug("Plan: step 2").unwrap(), "plan-step-2");
    }

    #[test]
    fn set_then_get_round_trips_and_orders_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        let s = set(&cfg, "flights-agent", "Flight Search", "line one").unwrap();
        assert_eq!(s.name, "flight-search");
        assert_eq!(s.agent, "flights-agent");
        assert_eq!(s.bytes, "line one".len() as u64);

        let path = dir.path().join("scratch/flights-agent/flight-search.md");
        let text = std::fs::read_to_string(&path).unwrap();
        let (yaml, _) = crate::fm::split_frontmatter(&text);
        let yaml = yaml.unwrap();
        let keys: Vec<&str> = yaml.lines().filter_map(|l| l.split(':').next()).collect();
        assert_eq!(
            keys,
            ["type", "name", "agent", "tags", "created", "updated"]
        );

        let view = get(&cfg, "flights-agent", "Flight Search").unwrap();
        assert_eq!(view.body, "line one");
        assert_eq!(view.item.name, "flight-search");
    }

    #[test]
    fn set_is_idempotent_on_an_identical_body() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        set(&cfg, "a", "n", "same").unwrap();
        let path = dir.path().join("scratch/a/n.md");
        let before = std::fs::read_to_string(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        set(&cfg, "a", "n", "same").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "identical body must not rewrite the file");
    }

    #[test]
    fn set_preserves_created_on_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        let first = set(&cfg, "a", "n", "one").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let second = set(&cfg, "a", "n", "two").unwrap();
        assert_eq!(first.created, second.created);
        assert!(second.updated > first.updated);
    }

    #[test]
    fn append_requires_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        let err = append(&cfg, "a", "missing", "x", AppendOpts::default()).unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "scratch not found: missing");
    }

    #[test]
    fn append_stamps_and_keeps_created() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        let created = set(&cfg, "a", "n", "base").unwrap().created;
        let after = append(
            &cfg,
            "a",
            "n",
            "note",
            AppendOpts {
                actor: Some("bob".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        let view = get(&cfg, "a", "n").unwrap();
        assert!(view.body.contains("base"));
        assert!(view.body.contains("note"));
        assert_eq!(after.created, created);
    }

    #[test]
    fn clear_deletes_and_is_not_found_after() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        set(&cfg, "a", "n", "x").unwrap();
        let name = clear(&cfg, "a", "n").unwrap();
        assert_eq!(name, "n");
        let err = get(&cfg, "a", "n").unwrap_err();
        assert_eq!(err.code(), 3);
        let err = clear(&cfg, "a", "n").unwrap_err();
        assert_eq!(err.to_string(), "scratch not found: n");
    }

    #[test]
    fn list_all_agents_sorts_by_updated_desc_then_name() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        set(&cfg, "a", "zeta", "x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        set(&cfg, "b", "alpha", "y").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        set(&cfg, "a", "beta", "z").unwrap();
        let views = list(&cfg, None, true, &Filter::unbounded()).unwrap();
        let names: Vec<&str> = views.iter().map(|v| v.item.name.as_str()).collect();
        assert_eq!(names, ["beta", "alpha", "zeta"]);
    }

    #[test]
    fn list_without_all_agents_and_no_identity_is_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        let err = list(&cfg, None, false, &Filter::unbounded()).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().starts_with("no agent identity"));
    }

    #[test]
    fn roster_validation_rejects_unknown_agents() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = cfg_at(dir.path());
        cfg.tasks.collections = vec!["alice".into()];
        let err = set(&cfg, "mallory", "n", "x").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "unknown owner: 'mallory'");
    }

    #[test]
    fn disabled_space_is_a_validation_error() {
        use crate::spaces::{SpaceSetting, Spaces};
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = cfg_at(dir.path());
        cfg.spaces =
            Spaces::resolve(dir.path(), &[(Space::Scratch, SpaceSetting::Disabled)]).unwrap();
        let err = set(&cfg, "a", "n", "x").unwrap_err();
        assert_eq!(err.to_string(), "space 'scratch' is disabled in [spaces]");
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn summary_counts_files_and_distinct_agents() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_at(dir.path());
        assert_eq!(summary(&cfg).files, 0);
        set(&cfg, "a", "one", "x").unwrap();
        set(&cfg, "a", "two", "y").unwrap();
        set(&cfg, "b", "three", "z").unwrap();
        let sum = summary(&cfg);
        assert_eq!(sum.files, 3);
        assert_eq!(sum.agents, 2);
    }
}
