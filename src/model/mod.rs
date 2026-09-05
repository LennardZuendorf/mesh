//! Typed views over frontmatter. Views validate and sort; the map is what gets written.

pub mod asset;
pub mod common;
pub mod memory;
pub mod note;
pub mod scratch;
pub mod task;

pub use asset::{AssetSidecar, AssetSummary, GcReport, ASSET_FIELDS};
pub use common::{FieldOrder, BASE_FIELDS};
pub use memory::{Memory, MemorySummary, MEMORY_FIELDS};
pub use note::{ForeignView, Note, NOTE_FIELDS, NOTE_TYPES};
pub use scratch::{Scratch, ScratchSummary, SCRATCH_FIELDS};
pub use task::{Task, TASK_FIELDS, TASK_PRIORITIES, TASK_STATUSES};
