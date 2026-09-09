//! Domain logic, one module per space plus the shared selection primitives.

pub mod activity;
pub mod assets;
pub mod context;
pub mod deps;
pub mod lenses;
pub mod memories;
pub mod notes;
pub mod owner;
pub mod scratch;
pub mod select;
pub mod tags;
pub mod tasks;
pub mod wikilinks;

/// Options shared by every `append` verb.
///
/// Foundation-owned (a deviation from ownership.md §2.2, which placed it in `domain::notes`)
/// so that the note, task, memory, scratch and asset agents cannot break each other.
#[derive(Clone, Debug, Default)]
pub struct AppendOpts {
    /// Append under `## {section}`, creating it at the end of the body when absent.
    pub section: Option<String>,
    /// Prefix the block with an attribution stamp.
    pub timestamp: bool,
    /// Who is running the command — never the entity's owner.
    pub actor: Option<String>,
}

pub use owner::{effective_owner, validate_owner};
pub use select::{select, Filter, FromMeta, SortKey, SortValue, Sortable};
pub use tags::{apply_tag_spec, TAG_SPEC_SEMANTICS};
pub use wikilinks::{backlinks_by_title, find_dangling, resolve_wikilinks, title_index};
