//! One test binary for the crate's integration tests (one link instead of one per file). `reflect_alloc` stays its own
//! binary: it installs a counting global allocator.
mod io_board;
mod online;
mod scenes;
mod standins;

use serde_json::Value;
use std::path::PathBuf;

/// The package folder at run time first: nextest's `--workspace-remap` sets it for an archive built on another machine.
fn manifest_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| env!("CARGO_MANIFEST_DIR").into(), PathBuf::from)
}

/// `tests/rec/<name>.jsonl`, one JSON value per line.
fn records(name: &str) -> Vec<Value> {
    let path = manifest_dir()
        .join("tests/rec")
        .join(format!("{name}.jsonl"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
