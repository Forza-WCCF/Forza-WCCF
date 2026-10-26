//! Not shipped. test_dhcphook.exe <path to wccfk.dll>: this program imports IpReleaseAddress and IpRenewAddress; once the
//! hook has loaded, both imports must point inside wccfk.dll. Nothing is called unless both do, so the real calls are
//! never made.
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
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return ExitCode::from(2);
    }
    let g = load(&args[1]);
    if !check(!g.is_null(), "wccfk.dll loads") {
        return ExitCode::from(1);
    }
    let calls: [(&str, check::IpCall); 2] = [
        ("IpReleaseAddress", IpReleaseAddress),
        ("IpRenewAddress", IpRenewAddress),
    ];
    imports_into(g, "wccfk.dll", "import points", &calls);
    done("dhcp hook test")
}
