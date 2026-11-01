//! One test binary for the crate's integration tests (one link instead of one per file). `run` and `procs` stay their
//! own binaries: they start copies of the test program by test name (`--exact`) with Prefer-System32 on.
mod arcade_room;
mod console;
mod debug;
mod lobby;
mod rec_card;
mod rec_conf;
mod rec_debug;
mod rec_keys;
mod rec_kit;
mod rec_logs;
mod sound;
