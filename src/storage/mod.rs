//! Durable storage primitives: atomic writes, locks, the sandbox and the walk.

pub mod atomic;
pub mod lock;
pub mod sandbox;
pub mod walk;

pub use atomic::atomic_write;
pub use lock::{acquire, create_lock, entity_lock, hold, is_stale, LockGuard};
pub use sandbox::{realpath, safe_resolve};
pub use walk::iter_md;
