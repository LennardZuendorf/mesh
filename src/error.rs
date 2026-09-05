//! The one error enum. `Display` is the exact stderr text; `code()` is the exit status.

use std::path::{Path, PathBuf};

/// The crate-wide result alias.
pub type Result<T> = std::result::Result<T, MeshError>;

/// Assemble the three-line missing-config text (surface.md §9).
fn config_missing_text(path: &Path) -> String {
    format!(
        "mesh: no config found at {}\n\
         run `mesh init` to create one (honours $MESH_CONFIG_PATH), or point $MESH_CONFIG_PATH \
         at an existing config.\n\
         required: [core].vault_path (path to your Markdown vault folder); [core].agent, \
         [search], and [tasks] are optional and default.",
        path.display()
    )
}

/// Render the `ambiguous slug` detail suffix: `: n-a, n-b`, or empty when there are no ids.
fn slug_detail(ids: &[String]) -> String {
    if ids.is_empty() {
        String::new()
    } else {
        format!(": {}", ids.join(", "))
    }
}

/// Every failure mesh can report. One variant per exit-code-bearing situation.
#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    /// Anything the user can fix by changing the input. Exit 2.
    #[error("{0}")]
    Validation(String),
    #[error("note not found: {0}")]
    NoteNotFound(String),
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("memory not found: {0}")]
    MemoryNotFound(String),
    #[error("asset not found: {0}")]
    AssetNotFound(String),
    #[error("scratch not found: {0}")]
    ScratchNotFound(String),
    #[error("seed not found: {0}")]
    SeedNotFound(String),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("ambiguous slug '{slug}'{}", slug_detail(ids))]
    AmbiguousSlug { slug: String, ids: Vec<String> },
    #[error("task {task_id} already claimed by {existing_owner}")]
    ClaimConflict {
        task_id: String,
        existing_owner: String,
    },
    /// A contended or unacquirable lock. Exit 4.
    #[error("{0}")]
    Lock(String),
    #[error("task {task_id} is blocked by {blockers}")]
    Blocked { task_id: String, blockers: String },
    #[error("{}", config_missing_text(path))]
    ConfigMissing { path: PathBuf },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A declined delete prompt. Exit 1, matching Click's `Abort`.
    #[error("Aborted!")]
    Aborted,
    /// A wrapper carrying `candidates` for the `--json` error envelope. Delegates everything.
    #[error("{inner}")]
    WithCandidates {
        inner: Box<MeshError>,
        candidates: Vec<String>,
    },
}

impl MeshError {
    /// Build the missing-config error for a resolved path.
    pub fn config_missing(path: impl Into<PathBuf>) -> Self {
        MeshError::ConfigMissing { path: path.into() }
    }

    /// Build a validation error from anything stringy.
    pub fn validation(msg: impl Into<String>) -> Self {
        MeshError::Validation(msg.into())
    }

    /// Attach up to five near-miss ids, surfaced as `candidates` in the JSON envelope.
    pub fn with_candidates(self, candidates: Vec<String>) -> Self {
        if candidates.is_empty() {
            return self;
        }
        match self {
            MeshError::WithCandidates { inner, .. } => {
                MeshError::WithCandidates { inner, candidates }
            }
            other => MeshError::WithCandidates {
                inner: Box::new(other),
                candidates,
            },
        }
    }

    /// The candidates attached by [`MeshError::with_candidates`], if any.
    pub fn candidates(&self) -> &[String] {
        match self {
            MeshError::WithCandidates { candidates, .. } => candidates,
            _ => &[],
        }
    }

    /// Peel the `candidates` wrapper.
    pub fn inner(&self) -> &MeshError {
        match self {
            MeshError::WithCandidates { inner, .. } => inner.inner(),
            other => other,
        }
    }

    /// The process exit status for this failure.
    pub fn code(&self) -> i32 {
        match self.inner() {
            MeshError::Io(_) | MeshError::Aborted => 1,
            MeshError::Validation(_)
            | MeshError::AmbiguousSlug { .. }
            | MeshError::ConfigMissing { .. } => 2,
            MeshError::NoteNotFound(_)
            | MeshError::TaskNotFound(_)
            | MeshError::MemoryNotFound(_)
            | MeshError::AssetNotFound(_)
            | MeshError::ScratchNotFound(_)
            | MeshError::SeedNotFound(_)
            | MeshError::ProjectNotFound(_) => 3,
            MeshError::ClaimConflict { .. } | MeshError::Lock(_) => 4,
            MeshError::Blocked { .. } => 5,
            MeshError::WithCandidates { .. } => 1,
        }
    }

    /// The `kind` token of the JSON error envelope.
    pub fn kind(&self) -> &'static str {
        match self.inner() {
            MeshError::ConfigMissing { .. } => "config_missing",
            MeshError::ClaimConflict { .. } => "claim_conflict",
            MeshError::Lock(_) => "lock_conflict",
            MeshError::AmbiguousSlug { .. } => "ambiguous_slug",
            MeshError::NoteNotFound(_)
            | MeshError::TaskNotFound(_)
            | MeshError::MemoryNotFound(_)
            | MeshError::AssetNotFound(_)
            | MeshError::ScratchNotFound(_)
            | MeshError::SeedNotFound(_)
            | MeshError::ProjectNotFound(_) => "not_found",
            MeshError::Validation(_) => "validation",
            MeshError::Blocked { .. } => "blocked",
            MeshError::Io(_) => "io_error",
            MeshError::Aborted => "error",
            MeshError::WithCandidates { .. } => "error",
        }
    }

    /// The `next_action` line of the JSON error envelope (map/mcp.md §5.4, plus `blocked`).
    pub fn next_action(&self) -> &'static str {
        match self.kind() {
            "config_missing" => "run `mesh init` to create a config, then retry",
            "claim_conflict" => "pick a different task, wait, or ask the named agent to release it",
            "lock_conflict" => "retry shortly — another process is mid-write on this entity",
            "ambiguous_slug" => "retry using one of the listed ids instead of the slug",
            "not_found" => "check the id and retry, or list to find the right one",
            "conflict" => "resolve the conflict and retry",
            "blocked" => "finish or cancel the blocking tasks, then retry",
            _ => "fix the input and retry",
        }
    }

    /// The structured envelope fields, in `_STRUCTURED_ATTRS` order.
    pub fn structured(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut out: Vec<(&'static str, serde_json::Value)> = Vec::new();
        match self.inner() {
            MeshError::TaskNotFound(id) => out.push(("task_id", id.as_str().into())),
            MeshError::ClaimConflict {
                task_id,
                existing_owner,
            } => {
                out.push(("task_id", task_id.as_str().into()));
                out.push(("existing_owner", existing_owner.as_str().into()));
            }
            MeshError::Blocked { task_id, .. } => out.push(("task_id", task_id.as_str().into())),
            MeshError::NoteNotFound(t)
            | MeshError::MemoryNotFound(t)
            | MeshError::AssetNotFound(t)
            | MeshError::ScratchNotFound(t) => out.push(("id_or_slug", t.as_str().into())),
            MeshError::AmbiguousSlug { slug, ids } => {
                out.push(("slug", slug.as_str().into()));
                out.push(("ids", ids.clone().into()));
            }
            MeshError::SeedNotFound(id) => out.push(("seed_id", id.as_str().into())),
            MeshError::ProjectNotFound(id) => out.push(("project_id", id.as_str().into())),
            MeshError::ConfigMissing { path } => {
                out.push(("cfg_path", path.display().to_string().into()));
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn display_strings_are_exact() {
        assert_eq!(
            MeshError::NoteNotFound("japan".into()).to_string(),
            "note not found: japan"
        );
        assert_eq!(
            MeshError::TaskNotFound("t-x".into()).to_string(),
            "task not found: t-x"
        );
        assert_eq!(
            MeshError::MemoryNotFound("m".into()).to_string(),
            "memory not found: m"
        );
        assert_eq!(
            MeshError::AssetNotFound("a".into()).to_string(),
            "asset not found: a"
        );
        assert_eq!(
            MeshError::ScratchNotFound("s".into()).to_string(),
            "scratch not found: s"
        );
        assert_eq!(
            MeshError::SeedNotFound("n-1".into()).to_string(),
            "seed not found: n-1"
        );
        assert_eq!(
            MeshError::ProjectNotFound("n-1".into()).to_string(),
            "project not found: n-1"
        );
        assert_eq!(
            MeshError::ClaimConflict {
                task_id: "t-1".into(),
                existing_owner: "bob".into()
            }
            .to_string(),
            "task t-1 already claimed by bob"
        );
        assert_eq!(
            MeshError::Blocked {
                task_id: "t-x".into(),
                blockers: "t-a, t-b".into()
            }
            .to_string(),
            "task t-x is blocked by t-a, t-b"
        );
    }

    #[test]
    fn ambiguous_slug_omits_empty_detail() {
        let with = MeshError::AmbiguousSlug {
            slug: "x".into(),
            ids: vec!["n-a".into(), "n-b".into()],
        };
        assert_eq!(with.to_string(), "ambiguous slug 'x': n-a, n-b");
        let without = MeshError::AmbiguousSlug {
            slug: "x".into(),
            ids: vec![],
        };
        assert_eq!(without.to_string(), "ambiguous slug 'x'");
    }

    #[test]
    fn config_missing_is_three_lines() {
        let e = MeshError::config_missing("/tmp/c.toml");
        let text = e.to_string();
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "mesh: no config found at /tmp/c.toml");
        assert_eq!(
            lines[1],
            "run `mesh init` to create one (honours $MESH_CONFIG_PATH), or point $MESH_CONFIG_PATH at an existing config."
        );
        assert_eq!(
            lines[2],
            "required: [core].vault_path (path to your Markdown vault folder); [core].agent, [search], and [tasks] are optional and default."
        );
        assert_eq!(e.code(), 2);
        assert_eq!(e.kind(), "config_missing");
    }

    #[test]
    fn codes_match_the_matrix() {
        assert_eq!(MeshError::Validation("x".into()).code(), 2);
        assert_eq!(MeshError::NoteNotFound("x".into()).code(), 3);
        assert_eq!(MeshError::Lock("lock is held: /x".into()).code(), 4);
        assert_eq!(
            MeshError::ClaimConflict {
                task_id: "t".into(),
                existing_owner: "o".into()
            }
            .code(),
            4
        );
        assert_eq!(
            MeshError::Blocked {
                task_id: "t".into(),
                blockers: "b".into()
            }
            .code(),
            5
        );
        assert_eq!(MeshError::Io(std::io::Error::other("boom")).code(), 1);
    }

    #[test]
    fn candidates_wrapper_is_transparent() {
        let e = MeshError::NoteNotFound("japan".into()).with_candidates(vec!["n-A".into()]);
        assert_eq!(e.to_string(), "note not found: japan");
        assert_eq!(e.code(), 3);
        assert_eq!(e.kind(), "not_found");
        assert_eq!(e.candidates(), ["n-A".to_string()]);
        assert_eq!(e.structured()[0].0, "id_or_slug");
    }

    #[test]
    fn next_action_never_reads_as_authorization() {
        for e in [
            MeshError::Validation("x".into()),
            MeshError::NoteNotFound("x".into()),
            MeshError::Lock("x".into()),
            MeshError::Blocked {
                task_id: "t".into(),
                blockers: "b".into(),
            },
            MeshError::config_missing("/x"),
        ] {
            let a = e.next_action().to_lowercase();
            for banned in ["not authorized", "denied", "permission", "forbidden"] {
                assert!(!a.contains(banned), "{a} contains {banned}");
            }
        }
    }
}
