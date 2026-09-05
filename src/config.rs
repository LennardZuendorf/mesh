//! Config loading: file, aliases, environment overlay, and the resolved space set.

use std::path::{Path, PathBuf};

use crate::error::{MeshError, Result};
use crate::spaces::{expand_user, Space, SpaceSetting, Spaces};

/// `$MESH_CONFIG_PATH`.
pub const ENV_CONFIG_PATH: &str = "MESH_CONFIG_PATH";
/// `$MESH_AGENT`.
pub const ENV_AGENT: &str = "MESH_AGENT";
/// `$MESH_VAULT`.
pub const ENV_VAULT: &str = "MESH_VAULT";
/// `$MESH_INDEXED_BIN`.
pub const ENV_INDEXED_BIN: &str = "MESH_INDEXED_BIN";

/// The built-in search corpus when `[search].spaces` is absent.
pub const DEFAULT_SEARCH_SPACES: [&str; 4] = ["notes", "tasks", "memories", "assets"];

/// `[core]`.
#[derive(Clone, Debug)]
pub struct CoreConfig {
    pub vault_path: PathBuf,
    pub agent: Option<String>,
}

/// `[search]`.
#[derive(Clone, Debug)]
pub struct SearchConfig {
    pub collection: Option<String>,
    pub hybrid: bool,
    pub threshold: f64,
    /// Whether `threshold` was physically present in the TOML.
    pub threshold_explicit: bool,
    pub engine: String,
    pub spaces: Vec<String>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            collection: None,
            hybrid: true,
            threshold: 0.65,
            threshold_explicit: false,
            engine: "auto".to_string(),
            spaces: DEFAULT_SEARCH_SPACES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

/// `[tasks]`.
#[derive(Clone, Debug, Default)]
pub struct TasksConfig {
    pub collections: Vec<String>,
    pub strict: bool,
}

/// The effective configuration for one invocation.
#[derive(Clone, Debug)]
pub struct Config {
    pub core: CoreConfig,
    pub search: SearchConfig,
    pub tasks: TasksConfig,
    pub spaces: Spaces,
}

impl Config {
    /// The acting identity, when one is configured.
    pub fn agent(&self) -> Option<&str> {
        self.core.agent.as_deref().filter(|a| !a.is_empty())
    }

    /// The vault root.
    pub fn vault(&self) -> &Path {
        &self.core.vault_path
    }

    /// A copy of this config acting as another identity (`session-start --owner`).
    pub fn with_agent(&self, agent: Option<&str>) -> Config {
        let mut next = self.clone();
        if let Some(a) = agent {
            next.core.agent = Some(a.to_string());
        }
        next
    }

    /// The root of a space, or a validation error when it is disabled.
    pub fn root(&self, space: Space) -> Result<&Path> {
        self.spaces.root(space)
    }
}

/// Where the config file lives: `--config`, then `$MESH_CONFIG_PATH`, then `~/.mesh/config.toml`.
pub fn resolve_config_path(flag: Option<&Path>) -> PathBuf {
    if let Some(p) = flag {
        return expand_user(&p.to_string_lossy());
    }
    if let Ok(env) = std::env::var(ENV_CONFIG_PATH) {
        if !env.is_empty() {
            return expand_user(&env);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mesh")
        .join("config.toml")
}

fn table_str(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn table_strings(table: &toml::Table, key: &str) -> Option<Vec<String>> {
    let items = table.get(key)?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
    )
}

/// Read and resolve the config. Unknown tables and keys are ignored, never rejected.
pub fn load_config(flag: Option<&Path>, vault_flag: Option<&Path>) -> Result<Config> {
    let path = resolve_config_path(flag);
    if !path.is_file() {
        return Err(MeshError::config_missing(path));
    }
    let text = std::fs::read_to_string(&path)?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| MeshError::Validation(format!("invalid config at {}: {e}", path.display())))?;
    from_table(&table, vault_flag)
}

/// Build a `Config` from an already-parsed TOML table (the seam `mesh config show` reuses).
pub fn from_table(table: &toml::Table, vault_flag: Option<&Path>) -> Result<Config> {
    let empty = toml::Table::new();
    let core = table
        .get("core")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty);
    let search_raw = table
        .get("search")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty);
    let tasks_raw = table
        .get("tasks")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty);
    let spaces_raw = table
        .get("spaces")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty);

    // vault_path, with the two legacy aliases and the two overrides above the file.
    let from_file = table_str(core, "vault_path")
        .or_else(|| table_str(core, "path"))
        .or_else(|| table_str(core, "tolaria_path"));
    let from_env = std::env::var(ENV_VAULT).ok().filter(|v| !v.is_empty());
    let raw_vault = match vault_flag {
        Some(p) => p.to_string_lossy().into_owned(),
        None => match from_env.or(from_file) {
            Some(v) => v,
            None => {
                return Err(MeshError::Validation(
                    "[core].vault_path is required".to_string(),
                ))
            }
        },
    };
    let vault_path = crate::storage::realpath(&expand_user(&raw_vault));
    if vault_path.exists() && !vault_path.is_dir() {
        return Err(MeshError::Validation(format!(
            "[core].vault_path is not a directory: {}",
            vault_path.display()
        )));
    }

    let agent = std::env::var(ENV_AGENT)
        .ok()
        .filter(|a| !a.is_empty())
        .or_else(|| table_str(core, "agent"));

    let defaults = SearchConfig::default();
    let search = SearchConfig {
        collection: table_str(search_raw, "collection"),
        hybrid: search_raw
            .get("hybrid")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.hybrid),
        threshold: search_raw
            .get("threshold")
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(defaults.threshold),
        threshold_explicit: search_raw.contains_key("threshold"),
        engine: table_str(search_raw, "engine").unwrap_or(defaults.engine),
        spaces: table_strings(search_raw, "spaces").unwrap_or(defaults.spaces),
    };

    let tasks = TasksConfig {
        collections: table_strings(tasks_raw, "collections").unwrap_or_default(),
        strict: tasks_raw
            .get("strict")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    let mut settings: Vec<(Space, SpaceSetting)> = Vec::new();
    for space in Space::ALL {
        let Some(value) = spaces_raw.get(space.name()) else {
            continue;
        };
        let setting = match value {
            toml::Value::Boolean(false) => SpaceSetting::Disabled,
            toml::Value::Boolean(true) => SpaceSetting::Default,
            toml::Value::String(s) => SpaceSetting::Path(s.clone()),
            _ => {
                return Err(MeshError::Validation(format!(
                    "[spaces].{} must be a path or false",
                    space.name()
                )))
            }
        };
        settings.push((space, setting));
    }
    let spaces = Spaces::resolve(&vault_path, &settings)?;

    Ok(Config {
        core: CoreConfig { vault_path, agent },
        search,
        tasks,
        spaces,
    })
}

/// What `mesh init` was asked to write.
#[derive(Clone, Debug)]
pub struct InitOptions {
    pub vault_path: PathBuf,
    pub agent: String,
    pub collections: Vec<String>,
    pub search_collection: Option<String>,
    pub hybrid: bool,
    pub threshold: Option<f64>,
    pub engine: String,
    pub spaces: bool,
    pub obsidian: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        InitOptions {
            vault_path: PathBuf::from("."),
            agent: "agent".to_string(),
            collections: Vec::new(),
            search_collection: None,
            hybrid: true,
            threshold: None,
            engine: "auto".to_string(),
            spaces: true,
            obsidian: false,
        }
    }
}

fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render the config file `mesh init` writes (surface.md §8.1).
pub fn render_config_toml(opts: &InitOptions) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("[core]".to_string());
    lines.push(format!(
        "vault_path = {}",
        toml_string(&opts.vault_path.to_string_lossy())
    ));
    lines.push(format!("agent = {}", toml_string(&opts.agent)));
    if opts.spaces {
        lines.push(String::new());
        lines.push("[spaces]".to_string());
        for space in Space::ALL {
            let value = if space == Space::Notes && opts.obsidian {
                ".".to_string()
            } else {
                space.default_dir().to_string()
            };
            lines.push(format!("{} = {}", space.name(), toml_string(&value)));
        }
    }
    lines.push(String::new());
    lines.push("[search]".to_string());
    lines.push(format!("hybrid = {}", opts.hybrid));
    if let Some(t) = opts.threshold {
        lines.push(format!("threshold = {t}"));
    }
    if let Some(c) = opts.search_collection.as_ref().filter(|c| !c.is_empty()) {
        lines.push(format!("collection = {}", toml_string(c)));
    }
    if opts.engine != "auto" {
        lines.push(format!("engine = {}", toml_string(&opts.engine)));
    }
    lines.push(String::new());
    lines.push("[tasks]".to_string());
    let roster: Vec<String> = opts.collections.iter().map(|c| toml_string(c)).collect();
    lines.push(format!("collections = [{}]", roster.join(", ")));
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
pub mod test_support {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// A config rooted at `vault` with the default space layout and agent `test-agent`.
    pub fn config_for(vault: &Path) -> Config {
        let spaces = Spaces::resolve(vault, &[]).expect("default spaces resolve");
        Config {
            core: CoreConfig {
                vault_path: crate::storage::realpath(vault),
                agent: Some("test-agent".to_string()),
            },
            search: SearchConfig::default(),
            tasks: TasksConfig::default(),
            spaces,
        }
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

    fn parse(text: &str) -> Result<Config> {
        let table: toml::Table = text.parse().unwrap();
        from_table(&table, None)
    }

    #[test]
    fn vault_aliases_have_a_precedence_order() {
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().display();
        let cfg = parse(&format!("[core]\npath = \"{v}\"\n")).unwrap();
        assert_eq!(cfg.vault(), crate::storage::realpath(dir.path()));
        let cfg = parse(&format!("[core]\ntolaria_path = \"{v}\"\n")).unwrap();
        assert_eq!(cfg.vault(), crate::storage::realpath(dir.path()));
        let cfg = parse(&format!(
            "[core]\nvault_path = \"{v}\"\npath = \"/nope\"\ntolaria_path = \"/nope2\"\n"
        ))
        .unwrap();
        assert_eq!(cfg.vault(), crate::storage::realpath(dir.path()));
    }

    #[test]
    fn threshold_explicitness_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().display();
        let cfg = parse(&format!("[core]\nvault_path = \"{v}\"\n")).unwrap();
        assert!(!cfg.search.threshold_explicit);
        assert!((cfg.search.threshold - 0.65).abs() < f64::EPSILON);
        let cfg = parse(&format!(
            "[core]\nvault_path = \"{v}\"\n[search]\nthreshold = 0.4\n"
        ))
        .unwrap();
        assert!(cfg.search.threshold_explicit);
        assert!((cfg.search.threshold - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_tables_and_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = parse(&format!(
            "[core]\nvault_path = \"{}\"\nfuture = 1\n[daemon]\nsocket = \"x\"\n",
            dir.path().display()
        ))
        .unwrap();
        assert_eq!(cfg.vault(), crate::storage::realpath(dir.path()));
    }

    #[test]
    fn a_non_directory_vault_is_a_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, "x").unwrap();
        let err = parse(&format!("[core]\nvault_path = \"{}\"\n", file.display())).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err
            .to_string()
            .starts_with("[core].vault_path is not a directory: "));
    }

    #[test]
    fn missing_config_reports_the_three_line_text() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let err = load_config(Some(&missing), None).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().starts_with("mesh: no config found at "));
        assert_eq!(err.to_string().lines().count(), 3);
    }

    #[test]
    fn spaces_table_disables_and_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = parse(&format!(
            "[core]\nvault_path = \"{}\"\n[spaces]\nnotes = \".\"\nscratch = false\n",
            dir.path().display()
        ))
        .unwrap();
        assert_eq!(
            cfg.root(Space::Notes).unwrap(),
            crate::storage::realpath(dir.path())
        );
        assert!(!cfg.spaces.enabled(Space::Scratch));
    }

    #[test]
    fn rendered_config_matches_the_documented_shape() {
        let opts = InitOptions {
            vault_path: PathBuf::from("/v"),
            agent: "a".into(),
            collections: vec!["a".into(), "b".into()],
            ..InitOptions::default()
        };
        let text = render_config_toml(&opts);
        assert_eq!(
            text,
            "[core]\nvault_path = \"/v\"\nagent = \"a\"\n\n[spaces]\nnotes = \"notes\"\n\
             tasks = \"tasks\"\nmemories = \"memories\"\nscratch = \"scratch\"\nassets = \"assets\"\n\n\
             [search]\nhybrid = true\n\n[tasks]\ncollections = [\"a\", \"b\"]\n"
        );
        assert!(
            !text.contains("threshold"),
            "threshold is omitted when unset"
        );
        let parsed: toml::Table = text.parse().unwrap();
        assert!(parsed.contains_key("core"));
    }

    #[test]
    fn rendered_config_honours_every_switch() {
        let opts = InitOptions {
            vault_path: PathBuf::from("/v"),
            agent: "a\"b".into(),
            search_collection: Some("c".into()),
            threshold: Some(0.4),
            engine: "builtin".into(),
            hybrid: false,
            spaces: false,
            obsidian: false,
            collections: vec![],
        };
        let text = render_config_toml(&opts);
        assert!(text.contains("agent = \"a\\\"b\""));
        assert!(text.contains("hybrid = false"));
        assert!(text.contains("threshold = 0.4"));
        assert!(text.contains("collection = \"c\""));
        assert!(text.contains("engine = \"builtin\""));
        assert!(!text.contains("[spaces]"));
        assert!(text.contains("collections = []"));
    }

    #[test]
    fn obsidian_points_notes_at_the_vault_root() {
        let opts = InitOptions {
            obsidian: true,
            ..InitOptions::default()
        };
        assert!(render_config_toml(&opts).contains("notes = \".\""));
    }
}
