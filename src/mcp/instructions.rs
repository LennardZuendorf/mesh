//! The MCP `instructions` block: a pure function of `Option<&Config>`, capped at 2048 bytes.
//!
//! Eight sections joined with a blank line, in the fixed order of `map/mcp.md` §6: what mesh
//! is, identity, valid owners, vault, recall, the tag trap, the coordination protocol, and how
//! to read results. Four of them are config-driven; every degraded variant still names
//! `mesh init`. Nothing here touches the filesystem, the process or a socket.

use crate::config::Config;
use crate::domain::TAG_SPEC_SEMANTICS;

/// The hard budget for the rendered block, in UTF-8 bytes. `tools/list` is paid on every
/// session before any work, and so is this.
pub const BUDGET_BYTES: usize = 2048;

/// How many roster identities are spelled out before the `(+n more)` tail.
pub const MAX_ROSTER_SHOWN: usize = 8;

/// Section 1 — what mesh is, plus the one sentence covering the three new spaces and the
/// which-space-wins rule (final.md §9.5).
const WHAT_MESH_IS: &str = "# mesh\n\
Three verbs over one shared Markdown vault: note, task, search. Markdown is the source of truth; mesh owns writes and fast reads — no separate memory store, no external task tracker.\n\
Also: memory (an agent's belief, recalled later), scratch (this session's state, nobody else's), asset (stored files) — note is durable knowledge the operator reads.";

/// Section 6 — the list-vs-comma-string trap, embedding `TAG_SPEC_SEMANTICS` verbatim.
const TAG_TRAP_HEAD: &str = "## Tag mutation\n\
`tags` is a list on note_new/task_new/memory_new. On note_update/task_update/memory_update it is a comma string: ";

/// Section 7 — the five coordination rules.
const COORDINATION: &str = "## Coordination protocol\n\
1. Check claimed_by before task_claim; pick another task if someone already holds it.\n\
2. Claim before starting work; release/finish/cancel when you stop.\n\
3. owner must match the roster when [tasks].collections is set (a value check, not an identity check) — claimed_by is never checked against it either way; neither proves who is actually calling.\n\
4. A `warnings` entry on creation flags a duplicate title — check the named id first.\n\
5. Prefer *_append over rewriting a body you did not write; use graph(direction=\"in\") or session_start to see who mentioned you.";

/// Section 8 — how to read what comes back.
const READING_RESULTS: &str = "## Reading results\n\
Every note/task has `owner`; tasks add `claimed_by` (null = open), `status`, `path`. Search hits add a score, plus `mode` (indexed/fallback) when query is set; creation responses add `warnings` (duplicate-title only). Withheld: delete, daemon controls, reindex, status, init, and task_release's --force.";

fn identity_section(cfg: Option<&Config>) -> String {
    let Some(cfg) = cfg else {
        return "## Your identity\n\
No config could be loaded — run `mesh init`, then restart this MCP session. No identity, roster, or vault path are known until then; pass an explicit claimer/owner to calls that need one."
            .to_string();
    };
    match cfg.agent() {
        None => "## Your identity\n\
No agent identity configured ([core].agent / $MESH_AGENT unset) — run `mesh init`, or pass an explicit claimer/owner to calls that need one (task_claim, task_release, note/task creation)."
            .to_string(),
        Some(agent) => format!(
            "## Your identity\n\
You are `{agent}` this session (from [core].agent / $MESH_AGENT) — tools with a claimer/owner param default to it when omitted."
        ),
    }
}

fn roster_section(cfg: Option<&Config>, shown: usize) -> String {
    let Some(cfg) = cfg else {
        return "## Valid owners\nNot known — see identity above.".to_string();
    };
    let roster = &cfg.tasks.collections;
    if roster.is_empty() {
        return "## Valid owners\n\
No roster configured ([tasks].collections is empty) — any owner string is accepted, so a typo'd identity will not be caught."
            .to_string();
    }
    let head: Vec<&str> = roster.iter().take(shown).map(String::as_str).collect();
    let mut body = head.join(", ");
    let hidden = roster.len().saturating_sub(head.len());
    if hidden > 0 {
        body.push_str(&format!(" (+{hidden} more)"));
    }
    format!("## Valid owners ([tasks].collections)\n{body}")
}

fn vault_section(cfg: Option<&Config>) -> String {
    match cfg {
        None => "## Vault\nNot known — see identity above.".to_string(),
        Some(cfg) => format!("## Vault\n{}", cfg.vault().display()),
    }
}

fn recall_section(cfg: Option<&Config>) -> String {
    let Some(cfg) = cfg else {
        return "## Recall\nUnknown — assume substring-only until config loads.".to_string();
    };
    if !cfg.search.hybrid {
        return "## Recall\n\
Substring fallback only ([search].hybrid = false) — search never calls indexed."
            .to_string();
    }
    let collection = cfg.search.collection.as_deref().unwrap_or("(unset)");
    format!(
        "## Recall\n\
Hybrid configured (collection {collection}), but search degrades silently to a substring scan when indexed is unreachable — call mesh_health, or check a hit's mode field, to see which path answered."
    )
}

/// Compose the eight sections at a given roster width.
fn compose(cfg: Option<&Config>, roster_shown: usize) -> String {
    let tag_trap = format!("{TAG_TRAP_HEAD}{TAG_SPEC_SEMANTICS}");
    [
        WHAT_MESH_IS.to_string(),
        identity_section(cfg),
        roster_section(cfg, roster_shown),
        vault_section(cfg),
        recall_section(cfg),
        tag_trap,
        COORDINATION.to_string(),
        READING_RESULTS.to_string(),
    ]
    .join("\n\n")
}

/// Truncate to at most `BUDGET_BYTES`, never mid-code-point.
fn clamp(text: String) -> String {
    if text.len() <= BUDGET_BYTES {
        return text;
    }
    let mut cut = BUDGET_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.get(..cut).unwrap_or_default().to_string()
}

/// Build the instructions block. Pure: same config in, same bytes out, ≤ [`BUDGET_BYTES`].
///
/// A long roster is the only elastic part, so it is what gives way first when the budget
/// binds; a pathological config is finally truncated on a code-point boundary.
pub fn build(cfg: Option<&Config>) -> String {
    for shown in (0..=MAX_ROSTER_SHOWN).rev() {
        let text = compose(cfg, shown);
        if text.len() <= BUDGET_BYTES {
            return text;
        }
    }
    clamp(compose(cfg, 0))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;

    /// The authorization denylist: none of these may appear in any rendered variant.
    const AUTH_DENYLIST: [&str; 12] = [
        "permission",
        "permit",
        "authoriz",
        "unauthoriz",
        "access denied",
        "forbidden",
        "not allowed",
        "enforc",
        "restrict",
        "privilege",
        "grant",
        "credential",
    ];

    fn variants() -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let base = config_for(dir.path());
        let mut no_agent = base.clone();
        no_agent.core.agent = None;
        let mut roster = base.clone();
        roster.tasks.collections = (0..25).map(|i| format!("agent-number-{i:02}")).collect();
        let mut hybrid = base.clone();
        hybrid.search.hybrid = true;
        hybrid.search.collection = Some("mesh-vault".to_string());
        let mut no_hybrid = base.clone();
        no_hybrid.search.hybrid = false;
        vec![
            build(None),
            build(Some(&base)),
            build(Some(&no_agent)),
            build(Some(&roster)),
            build(Some(&hybrid)),
            build(Some(&no_hybrid)),
        ]
    }

    #[test]
    fn every_variant_fits_the_budget() {
        for text in variants() {
            assert!(text.len() <= BUDGET_BYTES, "{} bytes", text.len());
        }
    }

    #[test]
    fn every_variant_is_denylist_clean() {
        for text in variants() {
            let lower = text.to_lowercase();
            for banned in AUTH_DENYLIST {
                assert!(!lower.contains(banned), "{banned} appears in the block");
            }
        }
    }

    #[test]
    fn no_notes_application_is_named() {
        for text in variants() {
            let lower = text.to_lowercase();
            assert!(!lower.contains("tolaria"));
            assert!(!lower.contains("obsidian"));
        }
    }

    #[test]
    fn every_degraded_variant_names_mesh_init() {
        let dir = tempfile::tempdir().unwrap();
        let mut no_agent = config_for(dir.path());
        no_agent.core.agent = None;
        for text in [build(None), build(Some(&no_agent))] {
            assert!(!text.is_empty());
            assert!(text.contains("mesh"));
            assert!(text.contains("mesh init"), "{text}");
        }
        // A healthy config still renders every section, it just has nothing to advise.
        for text in variants() {
            assert!(text.contains("# mesh"));
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn the_tag_sentence_is_embedded_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert!(build(Some(&cfg)).contains(TAG_SPEC_SEMANTICS));
        assert!(build(None).contains(TAG_SPEC_SEMANTICS));
    }

    #[test]
    fn the_block_is_config_driven() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = config_for(dir.path());
        a.core.agent = Some("flights-agent".to_string());
        let mut b = config_for(dir.path());
        b.core.agent = Some("notes-agent".to_string());
        let text_a = build(Some(&a));
        let text_b = build(Some(&b));
        assert_ne!(text_a, text_b);
        assert!(text_a.contains("flights-agent"));
        assert!(!text_a.contains("notes-agent"));
    }

    #[test]
    fn the_recall_section_says_which_path_answers() {
        let dir = tempfile::tempdir().unwrap();
        let mut off = config_for(dir.path());
        off.search.hybrid = false;
        let text = build(Some(&off));
        assert!(text.contains("never calls indexed"));
        assert!(text.contains("Substring fallback only"));

        let mut on = config_for(dir.path());
        on.search.hybrid = true;
        on.search.collection = Some("c".to_string());
        let text = build(Some(&on));
        assert!(text.contains("degrades"));
        assert!(text.contains("silently"));
        assert!(text.contains("mesh_health"));
    }

    #[test]
    fn a_long_roster_gives_way_before_the_budget_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.tasks.collections = (0..40)
            .map(|i| format!("a-very-long-agent-identity-number-{i:03}"))
            .collect();
        let text = build(Some(&cfg));
        assert!(text.len() <= BUDGET_BYTES);
        assert!(text.contains("more)"));
        // Every static section survives the trim.
        assert!(text.contains("## Coordination protocol"));
        assert!(text.contains("## Reading results"));
    }

    #[test]
    fn the_eight_sections_are_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let text = build(Some(&cfg));
        let heads = [
            "# mesh",
            "## Your identity",
            "## Valid owners",
            "## Vault",
            "## Recall",
            "## Tag mutation",
            "## Coordination protocol",
            "## Reading results",
        ];
        let mut at = 0usize;
        for head in heads {
            let found = text[at..].find(head).map(|i| i + at);
            assert!(found.is_some(), "{head} missing or out of order");
            at = found.unwrap_or(at);
        }
    }

    #[test]
    fn the_function_is_pure() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(build(Some(&cfg)), build(Some(&cfg)));
        assert_eq!(build(None), build(None));
    }
}
