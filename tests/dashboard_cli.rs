//! `mesh dashboard` — the terminal gate and the clap surface, driven through the real binary.
//!
//! Everything else about the dashboard is a pure function with unit tests beside it
//! (`src/cli/dashboard.rs`): the key map, the state transitions, the Drop guard and the
//! fail-soft refresh. A live loop needs a real terminal, and this harness deliberately has
//! none — which is what the gate below asserts.

mod common;

use common::VaultFixture;
use predicates::prelude::*;

/// No tty on stdin/stdout → exit 2, naming what is missing.
///
/// The harness pipes both streams, so this is the real headless path an agent hits, never a
/// faked one.
#[test]
fn a_headless_dashboard_exits_two_naming_the_terminal() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().arg("dashboard").output().expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "dashboard needs a terminal\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "the gate prints nothing on stdout"
    );
}

/// A zero tick would re-snapshot the vault in a busy loop; the parser refuses it before the
/// terminal gate runs.
#[test]
fn a_zero_interval_is_a_usage_error() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["dashboard", "--interval", "0"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--interval"), "{stderr}");
    assert!(!stderr.contains("needs a terminal"), "{stderr}");
}

/// The clap surface: the verb exists, and `--interval` is the one knob it takes.
#[test]
fn the_dashboard_help_renders_the_interval_flag() {
    let fixture = VaultFixture::new();
    fixture
        .cmd()
        .args(["dashboard", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: mesh dashboard"))
        .stdout(predicate::str::contains("--interval"));
}

// # Manual smoke (the plan's manual step — left for the controller)
//
// Against the real vault, in a real terminal:
//
//   mesh dashboard                 four panes fill, chrome reads `2s · mine:off · refreshed …`
//   m                              the badge flips to `mine:on` and the panes narrow at once
//   r                              the refresh clock jumps without waiting for the tick
//   Tab, then ↑ / ↓                the bright border moves; the arrows scroll only that pane
//   q, and separately Ctrl-C       the shell prompt returns in cooked mode, cursor visible
//   mesh dashboard --interval 1    the tick printed in the chrome tracks the flag
//   a vault edited in another tab  the change appears on the next tick, with no restart
