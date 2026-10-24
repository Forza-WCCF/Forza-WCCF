//! Not shipped: crashes at once with a write to address 0, 32-bit like the game, for the Launcher's crash snapshot test
//! (crates/launcher/tests/it/debug.rs: both memory snapshots of it).
fn main() {
    // an instruction, not a Rust null write: that is undefined behaviour the compiler may turn into another fault
    // SAFETY: none; the access violation at 0x00000000 is the point
    unsafe { std::arch::asm!("mov dword ptr [{0}], 1", in(reg) 0usize) };
}
