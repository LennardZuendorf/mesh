//! Roster validation at the core write boundary, for every space.

use crate::config::Config;
use crate::error::{MeshError, Result};

/// Reject an owner that is not in a non-empty `[tasks].collections`.
///
/// `None` is exempt (the default-to-config-agent path) and an empty roster disables the check.
/// This is a spelling check across a fleet's identities, never an authorisation boundary.
pub fn validate_owner(cfg: &Config, owner: Option<&str>) -> Result<()> {
    let roster = &cfg.tasks.collections;
    match owner {
        None => Ok(()),
        Some(o) if roster.is_empty() => {
            let _ = o;
            Ok(())
        }
        Some(o) if roster.iter().any(|c| c == o) => Ok(()),
        Some(o) => Err(MeshError::Validation(format!("unknown owner: '{o}'"))),
    }
}

/// The owner a write should record: the flag when given, else the configured agent.
pub fn effective_owner(cfg: &Config, flag: Option<&str>) -> Option<String> {
    match flag {
        Some(o) => Some(o.to_string()),
        None => cfg.agent().map(str::to_string),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;

    #[test]
    fn an_empty_roster_accepts_anything() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert!(validate_owner(&cfg, Some("anyone")).is_ok());
        assert!(validate_owner(&cfg, None).is_ok());
    }

    #[test]
    fn a_populated_roster_rejects_strangers() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.tasks.collections = vec!["alice".into(), "bob".into()];
        assert!(validate_owner(&cfg, Some("alice")).is_ok());
        assert!(validate_owner(&cfg, None).is_ok());
        let err = validate_owner(&cfg, Some("ghost-agent")).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "unknown owner: 'ghost-agent'");
    }

    #[test]
    fn effective_owner_falls_back_to_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(effective_owner(&cfg, None).as_deref(), Some("test-agent"));
        assert_eq!(effective_owner(&cfg, Some("x")).as_deref(), Some("x"));
    }
}
