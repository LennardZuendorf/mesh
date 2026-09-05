//! The global flags and the flag-placement contract (R6).

use std::path::PathBuf;

/// The root flags. Every command reads its merged copy off the `Ctx`.
#[derive(Clone, Debug, Default)]
pub struct GlobalOpts {
    pub json: bool,
    pub quiet: bool,
    pub owner: Option<String>,
    pub mine: bool,
    pub config: Option<PathBuf>,
    pub vault: Option<PathBuf>,
}

impl GlobalOpts {
    /// Merge a subcommand's local flags into the global ones.
    ///
    /// Booleans OR; `owner` is local-wins-else-global. `task update` and `memory update`
    /// deliberately pass `None` so their reassignment `--owner` is not folded in.
    pub fn coalesce(&mut self, json: bool, quiet: bool, owner: Option<String>) {
        self.json = self.json || json;
        self.quiet = self.quiet || quiet;
        if owner.is_some() {
            self.owner = owner;
        }
    }

    /// `--json` or `--quiet`: no prompts, no advisories that are not asked for.
    pub fn is_machine(&self) -> bool {
        self.json || self.quiet
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn booleans_or_and_owner_is_local_wins() {
        let mut g = GlobalOpts {
            json: true,
            owner: Some("global".into()),
            ..GlobalOpts::default()
        };
        g.coalesce(false, true, None);
        assert!(g.json && g.quiet);
        assert_eq!(g.owner.as_deref(), Some("global"));
        g.coalesce(false, false, Some("local".into()));
        assert_eq!(g.owner.as_deref(), Some("local"));
    }

    #[test]
    fn either_side_of_the_command_name_is_the_same() {
        let mut left = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        left.coalesce(false, false, None);
        let mut right = GlobalOpts::default();
        right.coalesce(true, false, None);
        assert_eq!(left.json, right.json);
        assert!(left.is_machine() && right.is_machine());
    }
}
