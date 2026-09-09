//! Space resolution: the five folders a vault is made of, and the sandbox they define.

use std::path::{Path, PathBuf};

use crate::error::{MeshError, Result};
use crate::storage::realpath;

/// The five addressable spaces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Space {
    Notes,
    Tasks,
    Memories,
    Scratch,
    Assets,
}

impl Space {
    /// Every space, in declaration order.
    pub const ALL: [Space; 5] = [
        Space::Notes,
        Space::Tasks,
        Space::Memories,
        Space::Scratch,
        Space::Assets,
    ];

    /// The `[spaces]` key and the name used in messages and payloads.
    pub fn name(self) -> &'static str {
        match self {
            Space::Notes => "notes",
            Space::Tasks => "tasks",
            Space::Memories => "memories",
            Space::Scratch => "scratch",
            Space::Assets => "assets",
        }
    }

    /// The built-in folder, relative to the vault root.
    pub fn default_dir(self) -> &'static str {
        self.name()
    }

    /// The id prefix entities in this space carry, if any.
    pub fn id_prefix(self) -> Option<&'static str> {
        match self {
            Space::Notes => Some("n-"),
            Space::Tasks => Some("t-"),
            Space::Memories => Some("m-"),
            Space::Assets => Some("a-"),
            Space::Scratch => None,
        }
    }

    /// Parse a space name.
    pub fn from_name(name: &str) -> Option<Space> {
        Space::ALL.into_iter().find(|s| s.name() == name)
    }

    fn index(self) -> usize {
        match self {
            Space::Notes => 0,
            Space::Tasks => 1,
            Space::Memories => 2,
            Space::Scratch => 3,
            Space::Assets => 4,
        }
    }
}

/// How one `[spaces]` key was configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceSetting {
    /// The key was absent — use the built-in default folder.
    Default,
    /// `false` — the space is disabled and every verb for it exits 2.
    Disabled,
    /// A path: absolute, relative to the vault, or `"."` for the vault root.
    Path(String),
}

/// The resolved roots, the sandbox they define, and the notes-walk exclusions.
#[derive(Clone, Debug)]
pub struct Spaces {
    roots: [Option<PathBuf>; 5],
    vault: PathBuf,
    sandbox: Vec<PathBuf>,
    notes_exclusions: Vec<PathBuf>,
}

impl Spaces {
    /// Resolve every space against `vault`, then run the three load-time validations.
    pub fn resolve(vault: &Path, settings: &[(Space, SpaceSetting)]) -> Result<Spaces> {
        let mut roots: [Option<PathBuf>; 5] = [None, None, None, None, None];
        for space in Space::ALL {
            let setting = settings
                .iter()
                .find(|(s, _)| *s == space)
                .map(|(_, v)| v.clone())
                .unwrap_or(SpaceSetting::Default);
            if let Some(root) = resolve_one(vault, space, &setting)? {
                if let Some(slot) = roots.get_mut(space.index()) {
                    *slot = Some(root);
                }
            }
        }

        // Duplicate roots.
        let enabled: Vec<(Space, PathBuf)> = Space::ALL
            .into_iter()
            .filter_map(|s| roots.get(s.index()).and_then(Clone::clone).map(|p| (s, p)))
            .collect();
        for (i, (space, root)) in enabled.iter().enumerate() {
            for (other, other_root) in enabled.iter().skip(i + 1) {
                if root == other_root {
                    return Err(MeshError::Validation(format!(
                        "spaces '{}' and '{}' resolve to the same directory: {}",
                        space.name(),
                        other.name(),
                        root.display()
                    )));
                }
            }
        }

        // Containment: when the notes space is the vault root, everything else lives under it.
        let notes_root = roots.first().and_then(Clone::clone);
        let mut notes_exclusions: Vec<PathBuf> = Vec::new();
        if let Some(notes) = &notes_root {
            for (space, root) in &enabled {
                if *space == Space::Notes || root == notes {
                    continue;
                }
                if root.starts_with(notes) {
                    notes_exclusions.push(root.clone());
                } else if notes == &realpath(vault) {
                    return Err(MeshError::Validation(format!(
                        "space '{}' must live inside {} when [spaces].notes is the vault root",
                        space.name(),
                        notes.display()
                    )));
                }
            }
        }
        notes_exclusions.sort();

        let mut sandbox: Vec<PathBuf> = enabled.iter().map(|(_, p)| p.clone()).collect();
        sandbox.sort();
        sandbox.dedup();

        Ok(Spaces {
            roots,
            vault: realpath(vault),
            sandbox,
            notes_exclusions,
        })
    }

    /// The resolved root of an enabled space.
    pub fn root(&self, space: Space) -> Result<&Path> {
        match self.roots.get(space.index()).and_then(Option::as_ref) {
            Some(p) => Ok(p.as_path()),
            None => Err(MeshError::Validation(format!(
                "space '{}' is disabled in [spaces]",
                space.name()
            ))),
        }
    }

    /// Whether a space is enabled.
    pub fn enabled(&self, space: Space) -> bool {
        self.roots.get(space.index()).is_some_and(Option::is_some)
    }

    /// Every enabled root; a path is inside the sandbox iff it is at or under one of these.
    pub fn sandbox(&self) -> &[PathBuf] {
        &self.sandbox
    }

    /// Subtrees a walk of `space` must skip (nested space roots).
    pub fn exclusions_for(&self, space: Space) -> &[PathBuf] {
        match space {
            Space::Notes => &self.notes_exclusions,
            _ => &[],
        }
    }

    /// The vault root.
    pub fn vault(&self) -> &Path {
        &self.vault
    }
}

fn resolve_one(vault: &Path, space: Space, setting: &SpaceSetting) -> Result<Option<PathBuf>> {
    let raw = match setting {
        SpaceSetting::Disabled => return Ok(None),
        SpaceSetting::Default => space.default_dir().to_string(),
        SpaceSetting::Path(p) => p.clone(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(Some(realpath(vault)));
    }
    let expanded = expand_user(trimmed);
    if expanded.is_absolute() {
        return Ok(Some(realpath(&expanded)));
    }
    let joined = realpath(&vault.join(&expanded));
    let vault_real = realpath(vault);
    if !joined.starts_with(&vault_real) {
        return Err(MeshError::Validation(format!(
            "space '{}' escapes the vault: {}",
            space.name(),
            joined.display()
        )));
    }
    Ok(Some(joined))
}

/// Expand a leading `~` against the home directory.
pub fn expand_user(text: &str) -> PathBuf {
    if text == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    match text.strip_prefix("~/") {
        Some(rest) => match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(text),
        },
        None => PathBuf::from(text),
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

    fn vault() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn defaults_match_the_python_layout() {
        let dir = vault();
        let s = Spaces::resolve(dir.path(), &[]).unwrap();
        assert_eq!(
            s.root(Space::Notes).unwrap(),
            realpath(&dir.path().join("notes"))
        );
        assert_eq!(
            s.root(Space::Tasks).unwrap(),
            realpath(&dir.path().join("tasks"))
        );
        assert_eq!(s.sandbox().len(), 5);
    }

    #[test]
    fn false_disables_a_space() {
        let dir = vault();
        let s = Spaces::resolve(dir.path(), &[(Space::Scratch, SpaceSetting::Disabled)]).unwrap();
        assert!(!s.enabled(Space::Scratch));
        let err = s.root(Space::Scratch).unwrap_err();
        assert_eq!(err.to_string(), "space 'scratch' is disabled in [spaces]");
        assert_eq!(err.code(), 2);
        assert_eq!(s.sandbox().len(), 4);
    }

    #[test]
    fn dot_means_the_vault_root_and_records_exclusions() {
        let dir = vault();
        let s = Spaces::resolve(
            dir.path(),
            &[(Space::Notes, SpaceSetting::Path(".".into()))],
        )
        .unwrap();
        assert_eq!(s.root(Space::Notes).unwrap(), realpath(dir.path()));
        let excl = s.exclusions_for(Space::Notes);
        assert_eq!(excl.len(), 4);
        assert!(excl.contains(&realpath(&dir.path().join("tasks"))));
    }

    #[test]
    fn duplicate_roots_are_rejected() {
        let dir = vault();
        let err = Spaces::resolve(
            dir.path(),
            &[(Space::Memories, SpaceSetting::Path("notes".into()))],
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().contains("resolve to the same directory"));
    }

    #[test]
    fn a_relative_root_may_not_escape() {
        let dir = vault();
        let err = Spaces::resolve(
            dir.path(),
            &[(Space::Assets, SpaceSetting::Path("../outside".into()))],
        )
        .unwrap_err();
        assert!(err.to_string().contains("escapes the vault"));
    }

    #[test]
    fn an_absolute_root_joins_the_sandbox() {
        let dir = vault();
        let other = vault();
        let s = Spaces::resolve(
            dir.path(),
            &[(
                Space::Assets,
                SpaceSetting::Path(other.path().to_string_lossy().into_owned()),
            )],
        )
        .unwrap();
        assert_eq!(s.root(Space::Assets).unwrap(), realpath(other.path()));
        assert!(s.sandbox().contains(&realpath(other.path())));
    }

    #[test]
    fn space_names_round_trip() {
        for space in Space::ALL {
            assert_eq!(Space::from_name(space.name()), Some(space));
        }
        assert_eq!(Space::from_name("nope"), None);
    }
}
