//! The publisher's network hosts, which the original programs still name (ALL.Net and AiMeDB servers, a proxy): the
//! game program's name lookups and WinInet calls fail for them before anything leaves the PC. [`install`] points the
//! program's imports at the checks; every other name is passed on unchanged.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
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
        name.len() >= d.len()
            && name[name.len() - d.len()..].eq_ignore_ascii_case(d)
            && (name.len() == d.len() || name[name.len() - d.len() - 1] == b'.')
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

/// The original of each hooked import, by [`HOOKS`] index.
static REAL: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

/// # Safety
/// `REAL[i]` holds the original of an import of type `F` (set before its slot points at the hook).
unsafe fn real<F: Copy>(i: usize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    // SAFETY: the caller's contract
    unsafe { std::mem::transmute_copy(&REAL[i].load(Relaxed)) }
}

type Gethostbyname = unsafe extern "system" fn(*const u8) -> *mut c_void;
unsafe extern "system" fn gethostbyname(name: *const u8) -> *mut c_void {
    if denied(name) {
        // SAFETY: plain Win32 call (WSASetLastError is SetLastError)
        unsafe { SetLastError(WSAHOST_NOT_FOUND) };
        return std::ptr::null_mut();
    }
    // SAFETY: the game's own call, passed on
    unsafe { real::<Gethostbyname>(0)(name) }
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
    unsafe { real::<Getaddrinfo>(1)(node, service, hints, res) }
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
    unsafe { real::<DnsQueryA>(2)(name, kind, options, extra, results, reserved) }
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
    unsafe { real::<InternetOpenA>(3)(agent, access, proxy, bypass, flags) }
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
    unsafe { real::<InternetOpenUrlA>(4)(inet, url, headers, len, flags, context) }
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
        real::<InternetConnectA>(5)(inet, server, port, user, password, service, flags, context)
    }
}

/// (DLL, import, ordinal if winsock's, hook), by [`REAL`] index: every resolver the games' programs import.
fn hooks() -> [(&'static str, &'static CStr, Option<u16>, usize); 6] {
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
    let hooks = hooks();
    let mut done = [false; 6];
    // SAFETY: the process's main module, loaded for its whole life
    let exe = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as *const u8
    };
    // SAFETY: `exe` is loaded for the process's life
    for d in unsafe { crate::imports(exe) } {
        let dll = d.name().to_bytes();
        for (i, (want, name, ordinal, hook)) in hooks.iter().enumerate() {
            if done[i] || !dll.eq_ignore_ascii_case(want.as_bytes()) {
                continue;
            }
            let slot = d
                .find(crate::By::Name(name))
                .or_else(|| ordinal.and_then(|o| d.find(crate::By::Ordinal(o))));
            if let Some(slot) = slot {
                // SAFETY: an import slot of the loaded program; the hook has the import's signature; the original is
                // kept before the slot points at the hook
                unsafe {
                    REAL[i].store(slot.read(), Relaxed);
                    done[i] = crate::redirect(slot, *hook).is_some();
                }
            }
        }
    }
    let list: Vec<String> = hooks
        .iter()
        .zip(done)
        .map(|((_, n, _, _), ok)| format!("{}={}", n.to_string_lossy(), u8::from(ok)))
        .collect();
    format!("publisher hosts blocked in: {}", list.join(" "))
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
}
