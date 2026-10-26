//! fakegame_iphlp.exe: fakegame.exe that also imports IpRenewAddress from IPHLPAPI.DLL (never calls it), for the
//! 2017-18 guard's tests (crates/launcher): as built it is an unguarded program; with its import renamed to wccfnet.dll,
//! as the 2017-18 setup renames the game's, it is a guarded one. Tests only, never shipped.
#[path = "fakegame.rs"]
mod game;

#[link(
    name = "iphlpapi.dll",
    kind = "raw-dylib",
    modifiers = "+verbatim",
    import_name_type = "undecorated"
)]
unsafe extern "system" {
    fn IpRenewAddress(info: *mut std::ffi::c_void) -> u32;
}

fn main() -> std::process::ExitCode {
    if std::hint::black_box(false) {
        // SAFETY: never runs: only the import is wanted
        unsafe { IpRenewAddress(std::ptr::null_mut()) };
    }
    game::main()
}
