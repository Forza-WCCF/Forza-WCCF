//! Not shipped. test_netguard.exe <path to netguard.dll>: this program imports the four IP helper calls netguard.dll
//! patches; after the load each import must point inside netguard.dll. Nothing is called unless every one does, so the
//! real calls are never made.
#[path = "../check.rs"]
mod check;
use check::{check, done, imports_into, load};
use std::ffi::c_void;
use std::process::ExitCode;

#[link(
    name = "iphlpapi.dll",
    kind = "raw-dylib",
    modifiers = "+verbatim",
    import_name_type = "undecorated"
)]
unsafe extern "system" {
    fn IpReleaseAddress(info: *mut c_void) -> u32;
    fn IpRenewAddress(info: *mut c_void) -> u32;
    fn CreateIpForwardEntry(row: *mut c_void) -> u32;
    fn SetIpForwardEntry(row: *mut c_void) -> u32;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return ExitCode::from(2);
    }
    let g = load(&args[1]);
    if !check(!g.is_null(), "netguard.dll loads") {
        return ExitCode::from(1);
    }
    let calls: [(&str, check::IpCall); 4] = [
        ("IpReleaseAddress", IpReleaseAddress),
        ("IpRenewAddress", IpRenewAddress),
        ("CreateIpForwardEntry", CreateIpForwardEntry),
        ("SetIpForwardEntry", SetIpForwardEntry),
    ];
    imports_into(g, "netguard.dll", "imports now point", &calls);
    done("netguard test")
}
