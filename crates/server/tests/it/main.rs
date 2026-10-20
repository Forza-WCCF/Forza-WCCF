//! One test binary for the crate's integration tests (one link instead of one per file). `relay_alloc` stays its own
//! binary: it installs a counting global allocator.
mod common;
mod desk;
mod gmsvr;
mod inbox;
#[cfg(target_os = "linux")]
mod load;
mod lobby;
mod relay;
mod rooms;
