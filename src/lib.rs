//! mesh — a mesh for multi-agent collaboration over a single Markdown folder.
//!
//! The library half: domain logic, storage primitives, the CLI tree and the MCP server. The
//! `mesh` binary is a thin `parse → dispatch → map error → exit` shell over it.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::string_slice
)]

pub mod cli;
pub mod config;
pub mod ctx;
pub mod domain;
pub mod error;
pub mod fm;
pub mod ids;
pub mod mcp;
pub mod model;
pub mod render;
pub mod search;
pub mod spaces;
pub mod storage;
pub mod text;
pub mod timefmt;

pub use error::{MeshError, Result};

/// The version `mesh --version` prints, bare, on stdout.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
