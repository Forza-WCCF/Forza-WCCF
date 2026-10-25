//! Not shipped. The 2017-18 server hook ([forza-hook e1718-server]) under Wine, in a folder with the server's winmm.dll,
//! winmm_orig.dll, netguard.dll and wccfnet.dll, run with `WINEDLLOVERRIDES="winmm,winmm_orig,netguard=n"` and
//! MXHOOK_LOG_DIR set. It imports what the 2017-18 control imports: timeGetTime from winmm, the DHCP release and renew
//! from IPHLPAPI (the original control; with the import renamed to wccfnet.dll, as setup makes the guarded control:
//! docs/SELF-HOSTING.md), gethostbyname and inet_addr from ws2_32 by ordinal. Checks that the hook forwards winmm, loaded
//! netguard.dll, holds both DHCP imports (called only then) and presents one network address, and that its log names the
//! edition and the two blocked calls.
#[path = "../check.rs"]
mod check;
use check::{check, done, imports_into, owner, target};
use std::ffi::c_void;
use std::process::ExitCode;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

#[link(
    name = "winmm.dll",
    kind = "raw-dylib",
    modifiers = "+verbatim",
    import_name_type = "undecorated"
)]
unsafe extern "system" {
    fn timeGetTime() -> u32;
}

// the same names as windows-sys's own iphlpapi imports (one symbol each in the link), so this is iphlpapi.dll
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

#[repr(C)]
struct Hostent {
    name: *const u8,
    aliases: *const *const u8,
    addrtype: i16,
    length: i16,
    addr_list: *const *const [u8; 4],
}

// by ordinal as the control; names of their own (an ordinal import is linked by its item name), as windows-sys
// imports these by name
#[link(name = "ws2_32.dll", kind = "raw-dylib", modifiers = "+verbatim")]
unsafe extern "system" {
    #[link_ordinal(115)]
    fn ws_startup(version: u16, data: *mut u8) -> i32;
    #[link_ordinal(57)]
    fn ws_gethostname(name: *mut u8, len: i32) -> i32;
    #[link_ordinal(52)]
    fn ws_gethostbyname(name: *const u8) -> *const Hostent;
    #[link_ordinal(11)]
    fn ws_inet_addr(cp: *const u8) -> u32;
}

fn module(name: &std::ffi::CStr) -> usize {
    // SAFETY: a NUL-terminated name
    unsafe { GetModuleHandleA(name.as_ptr().cast()) as usize }
}

fn main() -> ExitCode {
    let (hook, orig) = (module(c"winmm.dll"), module(c"winmm_orig.dll"));
    check(
        hook != 0 && orig != 0 && hook != orig,
        "winmm.dll (the hook) and winmm_orig.dll are both loaded",
    );
    check(
        owner(target(std::hint::black_box(
            timeGetTime as unsafe extern "system" fn() -> u32 as usize,
        ))) as usize
            == orig,
        "timeGetTime is forwarded to winmm_orig.dll",
    );
    // SAFETY: no arguments
    check(unsafe { timeGetTime() } != 0, "timeGetTime() answers");
    check(module(c"netguard.dll") != 0, "netguard.dll is loaded");
    let calls: [(&str, check::IpCall); 2] = [
        ("IpReleaseAddress", IpReleaseAddress),
        ("IpRenewAddress", IpRenewAddress),
    ];
    imports_into(hook as _, "winmm.dll (the hook)", "import points", &calls);

    let ip = std::env::var("MXHOOK_NET_IP").unwrap_or_else(|_| "127.0.0.1".into());
    let ip: std::net::Ipv4Addr = ip.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST);
    let mut wsa = [0u8; 512];
    let mut me = [0u8; 256];
    // SAFETY: a buffer larger than WSADATA, and one for the host name
    let named = unsafe {
        ws_startup(0x0202, wsa.as_mut_ptr()) == 0 && ws_gethostname(me.as_mut_ptr(), 255) == 0
    };
    if check(named, "WSAStartup and gethostname") {
        // SAFETY: a NUL-terminated name; a non-null answer is a hostent with a null-terminated address list
        let addrs = unsafe {
            let h = ws_gethostbyname(me.as_ptr());
            let mut v: Vec<std::net::Ipv4Addr> = vec![];
            while !h.is_null() && !(*h).addr_list.add(v.len()).read().is_null() {
                v.push((*(*h).addr_list.add(v.len()).read()).into());
            }
            v
        };
        check(
            addrs == [ip],
            &format!("gethostbyname(this PC) -> {addrs:?}, only {ip}"),
        );
    }
    // SAFETY: a NUL-terminated address
    let net = unsafe { ws_inet_addr(c"192.168.46.0".as_ptr().cast()) }.to_le_bytes();
    let o = ip.octets();
    check(
        net == [o[0], o[1], o[2], 0],
        &format!("inet_addr(\"192.168.46.0\") -> {net:?}, {ip}'s /24"),
    );

    let log = std::env::var_os("MXHOOK_LOG_DIR")
        .map(|d| std::path::Path::new(&d).join(format!("mxhook_{}.log", std::process::id())));
    let text = log
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    for line in [
        "edition: 2017-18 server",
        "IpReleaseAddress  BLOCKED",
        "IpRenewAddress    BLOCKED",
        "network address: this PC is presented",
    ] {
        check(
            text.contains(line),
            &format!("mxhook_<pid>.log (MXHOOK_LOG_DIR) has \"{line}\""),
        );
    }
    done("2017-18 server hook test")
}
