//! The integration-test harness. Foundation-owned; never edited by a phase-1 agent.
//!
//! Every test drives the real binary through `--config`, so no test mutates process-global
//! environment and the suite runs at default parallelism.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// The default config a fixture writes: agent `test-agent`, open roster.
pub const DEFAULT_CONFIG: &str = "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
                                  [tasks]\ncollections = []\n";

/// A temp vault plus its config file.
pub struct VaultFixture {
    pub dir: TempDir,
    pub vault: PathBuf,
    pub config: PathBuf,
    bin_dir: PathBuf,
}

impl VaultFixture {
    /// A temp vault with the default config.
    pub fn new() -> Self {
        Self::with(DEFAULT_CONFIG)
    }

    /// A temp vault with custom TOML. `{VAULT}` is replaced by the vault path.
    pub fn with(cfg_body: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).expect("create vault");
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let config = dir.path().join("config.toml");
        let body = cfg_body.replace("{VAULT}", &vault.to_string_lossy());
        std::fs::write(&config, body).expect("write config");
        VaultFixture {
            dir,
            vault,
            config,
            bin_dir,
        }
    }

    /// A copy of `tests/fixtures/python-vault`, with a config pointing at it.
    pub fn from_corpus() -> Self {
        let fixture = Self::new();
        copy_dir(&corpus_dir(), &fixture.vault);
        fixture
    }

    /// The `mesh` binary, pre-seeded with `--config <path>` and a clean environment.
    pub fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("mesh").expect("mesh binary");
        cmd.env_remove("MESH_CONFIG_PATH")
            .env_remove("MESH_AGENT")
            .env_remove("MESH_VAULT")
            .env_remove("MESH_INDEXED_BIN")
            .env("PATH", self.path_env())
            .arg("--config")
            .arg(&self.config);
        cmd
    }

    /// The `mesh` binary with no `--config`, for the missing-config paths.
    pub fn bare_cmd(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("mesh").expect("mesh binary");
        cmd.env_remove("MESH_AGENT")
            .env_remove("MESH_VAULT")
            .env_remove("MESH_INDEXED_BIN")
            .env("MESH_CONFIG_PATH", self.dir.path().join("missing.toml"))
            .env("PATH", self.path_env());
        cmd
    }

    /// Write a file inside the vault, creating parents.
    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.vault.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parents");
        }
        std::fs::write(path, contents).expect("write file");
    }

    /// Read a file inside the vault.
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.vault.join(rel)).expect("read file")
    }

    /// Every file in the vault, as sorted vault-relative paths.
    pub fn files(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        collect(&self.vault, &self.vault, &mut out);
        out.sort();
        out
    }

    /// Put a stub `indexed` on PATH that echoes `ndjson` and records its argv.
    pub fn fake_indexed(&self, ndjson: &str) -> &Self {
        let script = self.bin_dir.join("indexed");
        let log = self.dir.path().join("indexed-argv.log");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\ncat <<'NDJSON'\n{ndjson}\nNDJSON\n",
            log = log.display()
        );
        std::fs::write(&script, body).expect("write fake indexed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake indexed");
        }
        self
    }

    /// The argv lines the fake `indexed` recorded.
    pub fn indexed_argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("indexed-argv.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn path_env(&self) -> String {
        let existing = std::env::var("PATH").unwrap_or_default();
        format!("{}:{existing}", self.bin_dir.display())
    }
}

impl Default for VaultFixture {
    fn default() -> Self {
        Self::new()
    }
}

/// `tests/fixtures/python-vault` — the byte-frozen Python-written corpus.
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python-vault")
}

/// `tests/fixtures/golden` — the Python-produced reference payloads.
pub fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden")
}

/// Every `*.md` in the corpus, sorted, as absolute paths.
pub fn corpus_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    collect_paths(&corpus_dir(), &mut out);
    out.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("md"));
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

fn collect_paths(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_paths(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dest");
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).expect("copy file");
        }
    }
}
