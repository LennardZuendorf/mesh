//! Frontmatter: the ordered map, its scalars, the reader and the emitter.

pub mod doc;
pub mod emit;
pub mod load;
pub mod value;

pub use doc::{dump_doc, read_body, read_doc, read_meta_only, write_doc, Doc, Row, View};
pub use emit::emit_meta;
pub use load::{parse_meta, split_frontmatter};
pub use value::{Meta, Ts, TsValue, Value};
