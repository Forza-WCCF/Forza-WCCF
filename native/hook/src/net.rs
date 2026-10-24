//! The host's network, kept out of the game's reach.
//!
//! DHCP: on boot the game does a DHCP release/renew (IpReleaseAddress + IpRenewAddress) on a host NIC it enumerates,
//! normal for a cabinet that owns its network; on this PC it DROPS the real LAN. The stubs return NO_ERROR without
//! touching any adapter, so the game believes it configured its network and the host connection is never disturbed.
//!
//! The cabinet's network address (2026-10-03): client FUN_004584c0 / control FUN_00409a80 call gethostname ->
//! gethostbyname, then for each address test (addr & inet_addr("255.255.255.0")) == inet_addr("192.168.46.0"); every
//! address before a match sets error 8001 "Network address error (DHCP)", fatal for the cabinet. gethostbyname(<this
//! PC>) is answered with ONE address (MXHOOK_NET_IP, default 127.0.0.1) and inet_addr("192.168.46.0") with that
//! address's /24 network, so the check passes with an address that really exists on this PC. Nothing else is
//! changed. MXHOOK_NET_IP=off turns it off.

use crate::log::{Text, log};
use crate::patch::{Real, UNSURE, patch};
use forza_native::By;
use std::cell::UnsafeCell;
use std::ffi::{CStr, c_void};
use std::net::Ipv4Addr;
use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;
use windows_sys::Win32::Networking::WinSock::{AF_INET, HOSTENT};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

type IpAddr = unsafe extern "system" fn(*mut c_void) -> u32;
static REAL_IP_RELEASE: Real<IpAddr> = Real::new();
static REAL_IP_RENEW: Real<IpAddr> = Real::new();

unsafe extern "system" fn hook_ip_release_address(_info: *mut c_void) -> u32 {
    log!("IpReleaseAddress  BLOCKED (would release the host DHCP lease) -> NO_ERROR");
    0
}

unsafe extern "system" fn hook_ip_renew_address(_info: *mut c_void) -> u32 {
    log!("IpRenewAddress    BLOCKED (would renew the host DHCP lease) -> NO_ERROR");
    0
}

/// Hooks the two DHCP calls: (release took, renew took, unsure). Unsure: an iphlpapi import that could not be read or
/// patched, which may still reach the host's lease.
pub fn install_dhcp() -> (bool, bool, bool) {
    UNSURE.store(false, Relaxed);
    let rel = patch(
        &REAL_IP_RELEASE,
        "iphlpapi.dll",
        By::Name(c"IpReleaseAddress"),
        hook_ip_release_address,
    );
    let ren = patch(
        &REAL_IP_RENEW,
        "iphlpapi.dll",
        By::Name(c"IpRenewAddress"),
        hook_ip_renew_address,
    );
    (rel, ren, UNSURE.load(Relaxed))
}

// ---- the network address ----

type GetHostByName = unsafe extern "system" fn(*const u8) -> *mut HOSTENT;
type InetAddr = unsafe extern "system" fn(*const u8) -> u32;
type GetHostName = unsafe extern "system" fn(*mut u8, i32) -> i32;
static REAL_GETHOSTBYNAME: Real<GetHostByName> = Real::new();
static REAL_INET_ADDR: Real<InetAddr> = Real::new();

/// The one address the game is shown, as a hostent that points into itself.
struct Host {
    host: HOSTENT,
    list: [*mut i8; 2],
    alias: [*mut i8; 1],
    addr: [u8; 4],
    name: [u8; 256],
}

struct Shared(UnsafeCell<Host>);
// SAFETY: written in DllMain before the hooks are installed; afterwards only the host name is rewritten, as the
// game's own single caller asks (the original kit did the same)
unsafe impl Sync for Shared {}

static HOST: Shared = Shared(UnsafeCell::new(Host {
    host: HOSTENT {
        h_name: std::ptr::null_mut(),
        h_aliases: std::ptr::null_mut(),
        h_addrtype: 0,
        h_length: 0,
        h_addr_list: std::ptr::null_mut(),
    },
    list: [std::ptr::null_mut(); 2],
    alias: [std::ptr::null_mut(); 1],
    addr: [0; 4],
    name: [0; 256],
}));
/// MXHOOK_NET_IP once it is in force.
static NET_IP: OnceLock<Ipv4Addr> = OnceLock::new();

/// Installs the network address hooks unless MXHOOK_NET_IP=off; (gethostbyname, inet_addr) in force.
pub fn install_address() -> (bool, bool) {
    let v = std::env::var("MXHOOK_NET_IP")
        .ok()
        .filter(|v| !v.is_empty() && v.len() < 64);
    let v = v.as_deref().unwrap_or("127.0.0.1");
    if v.eq_ignore_ascii_case("off") {
        log!("network address: OFF (MXHOOK_NET_IP=off) - the game sees this PC's real addresses");
        return (false, false);
    }
    let Ok(ip) = v.parse::<Ipv4Addr>() else {
        log!("network address: MXHOOK_NET_IP=\"{v}\" is not an IPv4 address - OFF");
        return (false, false);
    };
    // SAFETY: DllMain, before the hooks below can run: nothing else touches HOST
    unsafe {
        let h = &mut *HOST.0.get();
        h.addr = ip.octets();
        h.list = [h.addr.as_mut_ptr().cast(), std::ptr::null_mut()];
        h.host = HOSTENT {
            h_name: h.name.as_mut_ptr().cast(),
            h_aliases: h.alias.as_mut_ptr(),
            h_addrtype: AF_INET as i16,
            h_length: 4,
            h_addr_list: h.list.as_mut_ptr(),
        };
    }
    let _ = NET_IP.set(ip);
    // ws2_32 ordinals: 52 gethostbyname, 11 inet_addr (both programs import them by ordinal)
    let g = patch(
        &REAL_GETHOSTBYNAME,
        "ws2_32.dll",
        By::Ordinal(52),
        hook_gethostbyname,
    );
    let i = patch(
        &REAL_INET_ADDR,
        "ws2_32.dll",
        By::Ordinal(11),
        hook_inet_addr,
    );
    if !g || !i {
        // half a hook would be worse than none: the one that took passes every call on
        log!(
            "network address: hook NOT installed (gethostbyname={} inet_addr={}) - OFF",
            u8::from(g),
            u8::from(i)
        );
        return (false, false);
    }
    ON.store(true, Relaxed);
    log!("network address: this PC is presented to the game as {ip} (MXHOOK_NET_IP)");
    (true, true)
}

static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Is `name` this PC (no name counts as this PC)? Keeps its name for the answer.
fn is_this_pc(name: *const u8) -> bool {
    let name = if name.is_null() {
        b"" as &[u8]
    } else {
        // SAFETY: the game's NUL-terminated host name
        unsafe { CStr::from_ptr(name.cast()) }.to_bytes()
    };
    if name.is_empty() {
        return true;
    }
    let mut me = [0u8; 256];
    // SAFETY: ws2_32 is loaded in the game; gethostname fills our buffer
    let ok = unsafe {
        let ws = GetModuleHandleA(c"ws2_32.dll".as_ptr().cast());
        match (!ws.is_null())
            .then(|| GetProcAddress(ws, c"gethostname".as_ptr().cast()))
            .flatten()
        {
            Some(f) => {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, GetHostName>(f)(
                    me.as_mut_ptr(),
                    me.len() as i32,
                ) == 0
            }
            None => false,
        }
    };
    me[255] = 0;
    let len = me.iter().position(|&b| b == 0).unwrap_or(255);
    if !ok || !name.eq_ignore_ascii_case(&me[..len]) {
        return false;
    }
    // SAFETY: the name buffer only; the game reads it after this call returns
    unsafe { (&mut (*HOST.0.get()).name)[..=len].copy_from_slice(&me[..=len]) };
    true
}

unsafe extern "system" fn hook_gethostbyname(name: *const u8) -> *mut HOSTENT {
    if ON.load(Relaxed) && is_this_pc(name) {
        let ip = NET_IP.get().copied().unwrap_or(Ipv4Addr::LOCALHOST);
        let shown = if name.is_null() {
            String::new()
        } else {
            Text(name).to_string()
        };
        log!(
            "gethostbyname(\"{shown}\") -> {ip} only  (MXHOOK_NET_IP; the real list is not shown to the game)"
        );
        // SAFETY: the static hostent, valid for the process's life
        return unsafe { &raw mut (*HOST.0.get()).host };
    }
    // SAFETY: the game's own call, passed on
    unsafe { REAL_GETHOSTBYNAME.get()(name) }
}

unsafe extern "system" fn hook_inet_addr(cp: *const u8) -> u32 {
    // SAFETY: the game's NUL-terminated address text
    if ON.load(Relaxed) && !cp.is_null() && unsafe { CStr::from_ptr(cp.cast()) } == c"192.168.46.0"
    {
        let o = NET_IP
            .get()
            .copied()
            .unwrap_or(Ipv4Addr::LOCALHOST)
            .octets();
        log!(
            "inet_addr(\"192.168.46.0\") -> {}.{}.{}.0  (the arcade network = MXHOOK_NET_IP's /24)",
            o[0],
            o[1],
            o[2]
        );
        return u32::from_le_bytes([o[0], o[1], o[2], 0]); // network order in memory
    }
    // SAFETY: the game's own call, passed on
    unsafe { REAL_INET_ADDR.get()(cp) }
}
