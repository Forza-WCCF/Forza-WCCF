//! One test binary for the crate's integration tests (one link instead of one per file). `reflect_alloc` stays its own
//! binary: it installs a counting global allocator.
mod io_board;
mod online;
mod scenes;
#[cfg(feature = "ui")]
mod snapshots;
mod standins;
#[cfg(feature = "ui")]
mod text;
