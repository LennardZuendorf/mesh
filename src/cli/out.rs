//! Every byte mesh prints goes through here: payloads, notices, prompts and errors.

use std::io::{BufRead, Write};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value as Json};

use crate::ctx::Ctx;
use crate::error::{MeshError, Result};
use crate::storage::lock::LOCK_RETRY_AFTER_MS;
use crate::timefmt::iso_z;

/// One compact JSON line, newline-terminated (overrides.md O3).
pub fn json_line(value: &Json) -> String {
    format!(
        "{}\n",
        serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
    )
}

fn out(text: &str) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(text.as_bytes());
}

fn err(text: &str) {
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = handle.write_all(text.as_bytes());
}

/// Print a line to stdout.
pub fn line(text: &str) {
    out(&format!("{text}\n"));
}

/// Class M — a mutation report. `--quiet` beats `--json`.
///
/// The JSON key order is `id`, then `fields` in order, then `updated`.
pub fn mutation(ctx: &Ctx, id: &str, verb: &str, fields: &[(&str, Json)], updated: DateTime<Utc>) {
    if ctx.g.quiet {
        line(id);
        return;
    }
    if ctx.g.json {
        let mut payload = Map::new();
        payload.insert("id".to_string(), Json::String(id.to_string()));
        for (key, value) in fields {
            payload.insert((*key).to_string(), value.clone());
        }
        payload.insert("updated".to_string(), Json::String(iso_z(&updated)));
        out(&json_line(&Json::Object(payload)));
        return;
    }
    line(&format!("{verb} {id}"));
}

/// Class M for the name-addressed spaces: the same precedence with a `name` key.
pub fn mutation_named(ctx: &Ctx, name: &str, verb: &str, fields: &[(&str, Json)]) {
    if ctx.g.quiet {
        line(name);
        return;
    }
    if ctx.g.json {
        let mut payload = Map::new();
        payload.insert("name".to_string(), Json::String(name.to_string()));
        for (key, value) in fields {
            payload.insert((*key).to_string(), value.clone());
        }
        out(&json_line(&Json::Object(payload)));
        return;
    }
    line(&format!("{verb} {name}"));
}

/// Class L — a row listing. `--json` beats `--quiet`.
pub fn rows(ctx: &Ctx, entries: &[Json], render: impl Fn(&Json) -> String) {
    if ctx.g.json {
        out(&json_line(&Json::Array(entries.to_vec())));
        return;
    }
    if ctx.g.quiet {
        for entry in entries {
            let id = entry.get("id").and_then(Json::as_str).unwrap_or("");
            line(id);
        }
        return;
    }
    for entry in entries {
        line(&render(entry));
    }
}

/// Class L — a single object. `--json` beats every other rendering.
pub fn object(ctx: &Ctx, value: &Json, render: impl Fn(&Json) -> String) {
    if ctx.g.json {
        out(&json_line(value));
        return;
    }
    line(&render(value));
}

/// An infrastructure notice on stderr. Suppressed by `--quiet`, never inside a payload.
pub fn notice(ctx: &Ctx, text: &str) {
    if ctx.g.quiet {
        return;
    }
    err(&format!("{text}\n"));
}

/// The delete decision table (surface.md §10).
pub fn delete_guard(ctx: &Ctx, id: &str, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    if ctx.is_machine() || !ctx.tty {
        return Err(MeshError::Validation(
            "refusing to delete on a non-interactive path; pass --force to confirm".to_string(),
        ));
    }
    out(&format!("Delete {id}? [y/N]: "));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let stdin = std::io::stdin();
    let read = stdin.lock().read_line(&mut answer);
    match read {
        Ok(_) if answer.trim().eq_ignore_ascii_case("y") => Ok(()),
        _ => Err(MeshError::Aborted),
    }
}

/// The JSON error envelope: `kind`, `message`, `next_action`, structured fields, then the
/// two additions.
pub fn error_envelope(e: &MeshError) -> Json {
    let mut payload = Map::new();
    payload.insert("kind".to_string(), Json::String(e.kind().to_string()));
    payload.insert("message".to_string(), Json::String(e.to_string()));
    payload.insert(
        "next_action".to_string(),
        Json::String(e.next_action().to_string()),
    );
    for (key, value) in e.structured() {
        payload.insert(key.to_string(), value);
    }
    let candidates = e.candidates();
    if !candidates.is_empty() {
        payload.insert(
            "candidates".to_string(),
            Json::Array(candidates.iter().map(|c| Json::String(c.clone())).collect()),
        );
    }
    if e.kind() == "lock_conflict" {
        payload.insert(
            "retry_after_ms".to_string(),
            Json::from(LOCK_RETRY_AFTER_MS),
        );
    }
    Json::Object(payload)
}

/// Report a failure: plain text on stderr, or the JSON envelope under `--json`.
pub fn render_error(ctx: &Ctx, e: &MeshError) {
    if ctx.g.json {
        err(&json_line(&error_envelope(e)));
    } else {
        err(&format!("{e}\n"));
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
    use crate::cli::globals::GlobalOpts;
    use crate::config::test_support::config_for;

    fn ctx(json: bool, quiet: bool) -> Ctx {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let g = GlobalOpts {
            json,
            quiet,
            ..GlobalOpts::default()
        };
        Ctx::with_config(g, config_for(dir.path()), false)
    }

    #[test]
    fn json_line_is_compact_and_terminated() {
        let value = serde_json::json!({"a": 1, "b": [1, 2]});
        assert_eq!(json_line(&value), "{\"a\":1,\"b\":[1,2]}\n");
    }

    #[test]
    fn json_preserves_insertion_order() {
        let mut map = Map::new();
        map.insert("z".into(), Json::from(1));
        map.insert("a".into(), Json::from(2));
        assert_eq!(json_line(&Json::Object(map)), "{\"z\":1,\"a\":2}\n");
    }

    #[test]
    fn the_delete_guard_refuses_machine_paths() {
        let err = delete_guard(&ctx(true, false), "n-1", false).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "refusing to delete on a non-interactive path; pass --force to confirm"
        );
        assert!(delete_guard(&ctx(true, false), "n-1", true).is_ok());
        // No tty and no machine flags is still a refusal.
        let err = delete_guard(&ctx(false, false), "n-1", false).unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn the_envelope_has_the_documented_key_order() {
        let e = MeshError::NoteNotFound("japan-visa".into()).with_candidates(vec!["n-CFCC".into()]);
        let payload = error_envelope(&e);
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["kind", "message", "next_action", "id_or_slug", "candidates"]
        );
        assert_eq!(
            payload["message"],
            Json::String("note not found: japan-visa".into())
        );
    }

    #[test]
    fn a_lock_conflict_carries_retry_after_ms() {
        let payload = error_envelope(&MeshError::Lock("lock is held: /x".into()));
        assert_eq!(payload["kind"], Json::String("lock_conflict".into()));
        assert_eq!(payload["retry_after_ms"], Json::from(LOCK_RETRY_AFTER_MS));
    }

    #[test]
    fn config_missing_carries_cfg_path() {
        let payload = error_envelope(&MeshError::config_missing("/tmp/c.toml"));
        assert_eq!(payload["kind"], Json::String("config_missing".into()));
        assert_eq!(payload["cfg_path"], Json::String("/tmp/c.toml".into()));
        assert!(payload["next_action"]
            .as_str()
            .unwrap()
            .contains("mesh init"));
    }
}
