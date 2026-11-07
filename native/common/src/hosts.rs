//! The publisher's network hosts, which the original programs still name (ALL.Net and AiMeDB servers, a proxy): the
//! game program's name lookups and WinInet calls fail for them before anything leaves the PC. [`install`] points the
//! program's imports at the checks; every other name is passed on unchanged. The 2017-18 guard also refuses the
//! publisher's addresses the programs hold as numbers as a destination of `connect` and `sendto`
//! ([`install_addresses`]).

use std::ffi::{CStr, c_void};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::Foundation::SetLastError;

/// The domains the original programs name, and their parents; a name is denied when it is one or ends in `.<one>`.
pub const DENIED: &[&str] = &[
    "naominet.jp",
    "my-aime.net",
    "sega.co.jp",
    "sega.jp",
    "sega.com",
];

/// Is `name` a denied host (case-insensitive, a trailing root dot ignored)?
fn is_denied(name: &[u8]) -> bool {
    let name = name.strip_suffix(b".").unwrap_or(name);
    DENIED.iter().any(|d| {
        let d = d.as_bytes();
        name.len()
            .checked_sub(d.len())
            .and_then(|cut| name.split_at_checked(cut))
            .is_some_and(|(head, tail)| {
                tail.eq_ignore_ascii_case(d) && head.last().is_none_or(|&c| c == b'.')
            })
    })
}

/// Does `text` (a host name, a URL, a proxy list) name a denied host anywhere?
pub fn mentions_denied(text: &[u8]) -> bool {
    text.split(|c| !(c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-'))
        .any(is_denied)
}

const WSAHOST_NOT_FOUND: u32 = 11001; // also getaddrinfo's EAI_NONAME
const DNS_ERROR_RCODE_NAME_ERROR: i32 = 9003;
const ERROR_INTERNET_NAME_NOT_RESOLVED: u32 = 12007;
/// The control's connect wrappers (0x40B710, 0x4E1810) close the socket and fail on any error but WOULDBLOCK and a few
/// retry codes; its sendto callers (0x433BF0, 0x4C1480) on any error.
const WSAECONNREFUSED: u32 = 10061;
const WSAEHOSTUNREACH: u32 = 10065;
const SOCKET_ERROR: i32 = -1;

static LOG: OnceLock<fn(&str)> = OnceLock::new();

/// True (and one log line) when the game's NUL-terminated `text` names a denied host.
fn denied(text: *const u8) -> bool {
    if text.is_null() {
        return false;
    }
    // SAFETY: a NUL-terminated string the game passes to the call
    let t = unsafe { CStr::from_ptr(text.cast()) }.to_bytes();
    if !mentions_denied(t) {
        return false;
    }
    if let Some(log) = LOG.get() {
        log(&format!(
            "blocked publisher host {}",
            String::from_utf8_lossy(t)
        ));
    }
    true
}

/// The original of each hooked import, by [`hooks`] index.
static REAL: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

/// # Safety
/// `slot` holds the original of an import of type `F` (set before its import slot points at the hook).
unsafe fn real<F: Copy>(slot: &AtomicUsize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    // SAFETY: the caller's contract
    unsafe { std::mem::transmute_copy(&slot.load(Relaxed)) }
}

type Gethostbyname = unsafe extern "system" fn(*const u8) -> *mut c_void;
unsafe extern "system" fn gethostbyname(name: *const u8) -> *mut c_void {
    if denied(name) {
        // SAFETY: plain Win32 call (WSASetLastError is SetLastError)
        unsafe { SetLastError(WSAHOST_NOT_FOUND) };
        return std::ptr::null_mut();
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<Gethostbyname>(&REAL[0])(name) }
}

type Getaddrinfo =
    unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32;
unsafe extern "system" fn getaddrinfo(
    node: *const u8,
    service: *const u8,
    hints: *const c_void,
    res: *mut *mut c_void,
) -> i32 {
    if denied(node) {
        // SAFETY: the game's result pointer, cleared as a failed call leaves it
        unsafe {
            if !res.is_null() {
                *res = std::ptr::null_mut();
            }
            SetLastError(WSAHOST_NOT_FOUND);
        }
        return WSAHOST_NOT_FOUND as i32;
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<Getaddrinfo>(&REAL[1])(node, service, hints, res) }
}

type DnsQueryA = unsafe extern "system" fn(
    *const u8,
    u16,
    u32,
    *mut c_void,
    *mut *mut c_void,
    *mut c_void,
) -> i32;
unsafe extern "system" fn dns_query_a(
    name: *const u8,
    kind: u16,
    options: u32,
    extra: *mut c_void,
    results: *mut *mut c_void,
    reserved: *mut c_void,
) -> i32 {
    if denied(name) {
        if !results.is_null() {
            // SAFETY: the game's result pointer
            unsafe { *results = std::ptr::null_mut() };
        }
        return DNS_ERROR_RCODE_NAME_ERROR;
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<DnsQueryA>(&REAL[2])(name, kind, options, extra, results, reserved) }
}

/// WinInet resolves inside its own DLL, so its entry points are checked instead: the proxy, the URL, the server.
type InternetOpenA =
    unsafe extern "system" fn(*const u8, u32, *const u8, *const u8, u32) -> *mut c_void;
unsafe extern "system" fn internet_open_a(
    agent: *const u8,
    access: u32,
    proxy: *const u8,
    bypass: *const u8,
    flags: u32,
) -> *mut c_void {
    if denied(proxy) {
        // SAFETY: plain Win32 call
        unsafe { SetLastError(ERROR_INTERNET_NAME_NOT_RESOLVED) };
        return std::ptr::null_mut();
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<InternetOpenA>(&REAL[3])(agent, access, proxy, bypass, flags) }
}

type InternetOpenUrlA =
    unsafe extern "system" fn(*mut c_void, *const u8, *const u8, u32, u32, usize) -> *mut c_void;
unsafe extern "system" fn internet_open_url_a(
    inet: *mut c_void,
    url: *const u8,
    headers: *const u8,
    len: u32,
    flags: u32,
    context: usize,
) -> *mut c_void {
    if denied(url) {
        // SAFETY: plain Win32 call
        unsafe { SetLastError(ERROR_INTERNET_NAME_NOT_RESOLVED) };
        return std::ptr::null_mut();
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<InternetOpenUrlA>(&REAL[4])(inet, url, headers, len, flags, context) }
}

type InternetConnectA = unsafe extern "system" fn(
    *mut c_void,
    *const u8,
    u16,
    *const u8,
    *const u8,
    u32,
    u32,
    usize,
) -> *mut c_void;
#[allow(clippy::too_many_arguments)] // the import's own signature
unsafe extern "system" fn internet_connect_a(
    inet: *mut c_void,
    server: *const u8,
    port: u16,
    user: *const u8,
    password: *const u8,
    service: u32,
    flags: u32,
    context: usize,
) -> *mut c_void {
    if denied(server) {
        // SAFETY: plain Win32 call
        unsafe { SetLastError(ERROR_INTERNET_NAME_NOT_RESOLVED) };
        return std::ptr::null_mut();
    }
    // SAFETY: the game's own call, passed on
    unsafe {
        real::<InternetConnectA>(&REAL[5])(
            inet, server, port, user, password, service, flags, context,
        )
    }
}

/// The publisher's addresses the original programs hold as numbers: the 2017-18 control's built-in game server
/// (control_Release.exe 0x43F50A, string at 0x63723C), dialled when ctrl_user_option.conf has no GAME_SVR_IP.
pub const BLOCKED_ADDRS: [Ipv4Addr; 1] = [Ipv4Addr::new(157, 109, 70, 71)];

/// Whether each [`BLOCKED_ADDRS`] entry was logged in this process.
static ADDR_LOGGED: [AtomicBool; BLOCKED_ADDRS.len()] =
    [const { AtomicBool::new(false) }; BLOCKED_ADDRS.len()];

/// The [`BLOCKED_ADDRS`] index and port of a socket address (`AF_INET`, or `AF_INET6` holding a mapped IPv4) that is
/// one of them; None for every other address and for anything too short to read.
#[cfg_attr(all(test, not(debug_assertions)), no_panic::no_panic)]
fn blocked_dest(sa: &[u8]) -> Option<(usize, u16)> {
    let family = u16::from_le_bytes(sa.get(..2)?.try_into().ok()?);
    let port = u16::from_be_bytes(sa.get(2..4)?.try_into().ok()?);
    let ip = match family {
        2 => Ipv4Addr::from(<[u8; 4]>::try_from(sa.get(4..8)?).ok()?),
        23 => Ipv6Addr::from(<[u8; 16]>::try_from(sa.get(8..24)?).ok()?).to_ipv4_mapped()?,
        _ => return None,
    };
    BLOCKED_ADDRS
        .iter()
        .position(|b| *b == ip)
        .map(|i| (i, port))
}

/// True when the game's socket address `name` of `len` bytes is a blocked destination; the first time for each
/// address, one log line.
fn refused(call: &str, name: *const u8, len: i32) -> bool {
    if name.is_null() || len <= 0 {
        return false;
    }
    // SAFETY: the address of `len` bytes the game passes to the call
    let sa = unsafe { std::slice::from_raw_parts(name, len as usize) };
    let Some((i, port)) = blocked_dest(sa) else {
        return false;
    };
    if let (Some(logged), Some(addr)) = (ADDR_LOGGED.get(i), BLOCKED_ADDRS.get(i))
        && !logged.swap(true, Relaxed)
        && let Some(log) = LOG.get()
    {
        log(&format!(
            "blocked publisher address {addr}:{port} ({call}), later attempts refused unlogged"
        ));
    }
    true
}

/// The original of each address-checked import, by [`addr_hooks`] index.
static REAL_ADDR: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

type Connect = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
unsafe extern "system" fn connect(s: usize, name: *const u8, len: i32) -> i32 {
    if refused("connect", name, len) {
        // SAFETY: plain Win32 call (WSASetLastError is SetLastError)
        unsafe { SetLastError(WSAECONNREFUSED) };
        return SOCKET_ERROR;
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<Connect>(&REAL_ADDR[0])(s, name, len) }
}

type Sendto = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
unsafe extern "system" fn sendto(
    s: usize,
    buf: *const u8,
    len: i32,
    flags: i32,
    to: *const u8,
    tolen: i32,
) -> i32 {
    if refused("sendto", to, tolen) {
        // SAFETY: plain Win32 call (WSASetLastError is SetLastError)
        unsafe { SetLastError(WSAEHOSTUNREACH) };
        return SOCKET_ERROR;
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<Sendto>(&REAL_ADDR[1])(s, buf, len, flags, to, tolen) }
}

/// (DLL, import, ordinal, hook), by [`REAL_ADDR`] index: the calls that take a destination address which the 2017-18
/// programs import (both by ordinal; neither imports WSAConnect, WSASendTo or ConnectEx).
fn addr_hooks() -> [Hook; 2] {
    [
        (
            "ws2_32.dll",
            c"connect",
            Some(4),
            connect as Connect as usize,
        ),
        ("ws2_32.dll", c"sendto", Some(20), sendto as Sendto as usize),
    ]
}

/// (DLL, import, ordinal if winsock's, hook).
type Hook = (&'static str, &'static CStr, Option<u16>, usize);

/// (DLL, import, ordinal if winsock's, hook), by [`REAL`] index: every resolver the games' programs import.
fn hooks() -> [Hook; 6] {
    [
        (
            "ws2_32.dll",
            c"gethostbyname",
            Some(52),
            gethostbyname as Gethostbyname as usize,
        ),
        (
            "ws2_32.dll",
            c"getaddrinfo",
            None,
            getaddrinfo as Getaddrinfo as usize,
        ),
        (
            "dnsapi.dll",
            c"DnsQuery_A",
            None,
            dns_query_a as DnsQueryA as usize,
        ),
        (
            "wininet.dll",
            c"InternetOpenA",
            None,
            internet_open_a as InternetOpenA as usize,
        ),
        (
            "wininet.dll",
            c"InternetOpenUrlA",
            None,
            internet_open_url_a as InternetOpenUrlA as usize,
        ),
        (
            "wininet.dll",
            c"InternetConnectA",
            None,
            internet_connect_a as InternetConnectA as usize,
        ),
    ]
}

/// Points the program's imports of the resolvers at the checks, which log through `log`; returns the summary line
/// (`publisher hosts blocked in: gethostbyname=1 ...`, 0 = not imported or not patched).
pub fn install(log: fn(&str)) -> String {
    let _ = LOG.set(log);
    format!("publisher hosts blocked in: {}", patch(&hooks(), &REAL))
}

/// Points the program's imports of `connect` and `sendto` at the address checks, which log through `log`; returns the
/// summary line (`publisher addresses blocked in: connect=1 sendto=1`, 0 = not imported or not patched).
pub fn install_addresses(log: fn(&str)) -> String {
    let _ = LOG.set(log);
    format!(
        "publisher addresses blocked in: {}",
        patch(&addr_hooks(), &REAL_ADDR)
    )
}

/// Points the main program's import of each of `hooks` at its hook, the original kept in `real` at the same index;
/// returns `name=1` (patched) or `name=0` for each.
fn patch(hooks: &[Hook], real: &[AtomicUsize]) -> String {
    let mut done = vec![false; hooks.len()];
    // SAFETY: the process's main module, loaded for its whole life
    let exe = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as *const u8
    };
    // SAFETY: `exe` is loaded for the process's life
    for d in unsafe { crate::imports(exe) } {
        let dll = d.name().to_bytes();
        for ((want, name, ordinal, hook), (done, real)) in
            hooks.iter().zip(done.iter_mut().zip(real))
        {
            if *done || !dll.eq_ignore_ascii_case(want.as_bytes()) {
                continue;
            }
            let slot = d
                .find(crate::By::Name(name))
                .or_else(|| ordinal.and_then(|o| d.find(crate::By::Ordinal(o))));
            if let Some(slot) = slot {
                // SAFETY: an import slot of the loaded program; the hook has the import's signature; the original is
                // kept before the slot points at the hook
                unsafe {
                    real.store(slot.read(), Relaxed);
                    *done = crate::redirect(slot, *hook).is_some();
                }
            }
        }
    }
    let list: Vec<String> = hooks
        .iter()
        .zip(done)
        .map(|((_, n, _, _), ok)| format!("{}={}", n.to_string_lossy(), u8::from(ok)))
        .collect();
    list.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_the_publishers_hosts_only() {
        for t in [
            "aime.naominet.jp",
            "NAOMINET.JP",
            "naominet.jp.",
            "ib.naominet.jp",
            "http://naominet.jp/sys/servlet/PowerOn",
            "http://hm-proxy2.am3.sega.co.jp:10443",
            "sega.com",
            "www.my-aime.net",
        ] {
            assert!(mentions_denied(t.as_bytes()), "{t}");
        }
        for t in [
            "notnaominet.jp",
            "naominet.jp.evil.example",
            "lobby.wccf.it",
            "relay.wccf.it",
            "localhost",
            "127.0.0.1",
            "",
            "http://lobby.wccf.it:20050/naominet",
        ] {
            assert!(!mentions_denied(t.as_bytes()), "{t}");
        }
    }

    #[test]
    fn a_hooked_call_fails_a_denied_name_and_passes_the_rest() {
        unsafe extern "system" fn fake(_: *const u8) -> *mut c_void {
            8 as *mut c_void // stands in for a real answer
        }
        REAL[0].store(fake as Gethostbyname as usize, Relaxed);
        // SAFETY: NUL-terminated names; REAL[0] holds a function of the hook's type
        unsafe {
            assert!(gethostbyname(c"x.naominet.jp".as_ptr().cast()).is_null());
            assert_eq!(
                windows_sys::Win32::Foundation::GetLastError(),
                WSAHOST_NOT_FOUND
            );
            assert_eq!(gethostbyname(c"lobby.wccf.it".as_ptr().cast()) as usize, 8);
        }
    }

    /// An `AF_INET` socket address, as winsock takes it.
    fn v4(ip: [u8; 4], port: u16) -> [u8; 16] {
        let mut a = [0; 16];
        a[..2].copy_from_slice(&2u16.to_le_bytes());
        a[2..4].copy_from_slice(&port.to_be_bytes());
        a[4..8].copy_from_slice(&ip);
        a
    }

    /// An `AF_INET6` socket address of `ip`.
    fn v6(ip: Ipv6Addr, port: u16) -> [u8; 28] {
        let mut a = [0; 28];
        a[..2].copy_from_slice(&23u16.to_le_bytes());
        a[2..4].copy_from_slice(&port.to_be_bytes());
        a[8..24].copy_from_slice(&ip.octets());
        a
    }

    #[test]
    fn refuses_the_publishers_address_only() {
        let publisher = BLOCKED_ADDRS[0];
        assert_eq!(
            blocked_dest(&v4(publisher.octets(), 17182)),
            Some((0, 17182))
        );
        let mapped = v6(publisher.to_ipv6_mapped(), 17182);
        assert_eq!(blocked_dest(&mapped), Some((0, 17182)));
        assert_eq!(blocked_dest(&mapped[..16]), None); // too short to hold the address
        for (ip, port) in [
            ([127, 0, 0, 1], 20050),     // loopback: the game server stand-in
            ([127, 0, 0, 1], 20002),     // the control
            ([192, 168, 46, 254], 80),   // the game's own LAN
            ([10, 0, 0, 2], 20003),      // a LAN address
            ([157, 109, 70, 70], 17182), // its neighbours
            ([157, 109, 70, 72], 17182),
            ([0, 0, 0, 0], 0),
        ] {
            assert_eq!(blocked_dest(&v4(ip, port)), None, "{ip:?}");
            let m = v6(Ipv4Addr::from(ip).to_ipv6_mapped(), port);
            assert_eq!(blocked_dest(&m), None, "mapped {ip:?}");
        }
        assert_eq!(blocked_dest(&v6(Ipv6Addr::LOCALHOST, 20050)), None);
        assert_eq!(blocked_dest(&[]), None);
    }

    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    fn count(line: &str) {
        if line.starts_with("blocked publisher address 157.109.70.71:17182 (connect)") {
            LOGGED.fetch_add(1, Relaxed);
        }
    }

    #[test]
    fn a_hooked_connect_fails_the_publishers_address_once_logged() {
        unsafe extern "system" fn fake_connect(_: usize, _: *const u8, _: i32) -> i32 {
            0
        }
        unsafe extern "system" fn fake_sendto(
            _: usize,
            _: *const u8,
            len: i32,
            _: i32,
            _: *const u8,
            _: i32,
        ) -> i32 {
            len
        }
        let _ = LOG.set(count);
        REAL_ADDR[0].store(fake_connect as Connect as usize, Relaxed);
        REAL_ADDR[1].store(fake_sendto as Sendto as usize, Relaxed);
        let publisher = v4(BLOCKED_ADDRS[0].octets(), 17182);
        let mapped = v6(BLOCKED_ADDRS[0].to_ipv6_mapped(), 17182);
        let standin = v4([127, 0, 0, 1], 20050);
        let n = |a: &[u8]| a.len() as i32;
        // SAFETY: socket addresses of the given length; REAL_ADDR holds functions of the hooks' types
        unsafe {
            for _ in 0..3 {
                assert_eq!(connect(1, publisher.as_ptr(), n(&publisher)), SOCKET_ERROR);
                assert_eq!(
                    windows_sys::Win32::Foundation::GetLastError(),
                    WSAECONNREFUSED
                );
            }
            assert_eq!(connect(1, mapped.as_ptr(), n(&mapped)), SOCKET_ERROR);
            assert_eq!(connect(1, standin.as_ptr(), n(&standin)), 0);
            assert_eq!(connect(1, std::ptr::null(), 0), 0);
            let buf = [0u8; 8];
            assert_eq!(
                sendto(1, buf.as_ptr(), 8, 0, publisher.as_ptr(), n(&publisher)),
                SOCKET_ERROR
            );
            assert_eq!(
                windows_sys::Win32::Foundation::GetLastError(),
                WSAEHOSTUNREACH
            );
            assert_eq!(
                sendto(1, buf.as_ptr(), 8, 0, standin.as_ptr(), n(&standin)),
                8
            );
            assert_eq!(sendto(1, buf.as_ptr(), 8, 0, std::ptr::null(), 0), 8);
        }
        assert_eq!(LOGGED.load(Relaxed), 1); // one line for the address, whatever the call
    }
}
