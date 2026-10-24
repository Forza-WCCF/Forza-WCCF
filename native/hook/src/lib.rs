//! The hook in the game's programs, the PC stand-in for the cabinet's hardware: one source, three DLLs (Cargo.toml's
//! features). Loaded only by the game's programs, never injected; it patches the import table of the program that
//! loaded it, nothing else. What it does:
//!  - logs every device open, file mapping and DeviceIoControl and passes each on ([`devices`]); with
//!    MXHOOK_RINGEDGE=1 it stands in for the RingEdge devices;
//!  - serves the serial ports MXHOOK_COM names from the card reader's and the I/O board's named pipes ([`serial`]);
//!  - blocks the DHCP release and renew on the host's network card, which dropped the player's LAN, and presents this
//!    PC to the game with one address in the cabinet's network ([`net`]);
//!  - 2010-11: answers the server's start of logowin.exe itself ([`logowin`]) and gives the server the players'
//!    messages only whole ([`whole`]);
//!  - the 2010-11 server under Wine: loads netguard.dll and fixes three faults of control ([`server`]).
//!
//! The portable parts (the batch rules, the fake BIOS area, MXHOOK_COM, the logowin test, the server fixes' rules) build
//! and test on any host; the rest is for the game's 32-bit Windows.
#![cfg_attr(not(target_arch = "x86"), allow(dead_code))]

#[cfg(all(
    target_arch = "x86",
    not(any(feature = "e1011", feature = "e1718", feature = "e1011-server"))
))]
compile_error!(
    "forza-hook is built with --features e1011 (wccfk.dll), e1718 (winmm.dll) or e1011-server (the server's winmm.dll)"
);
#[cfg(any(
    all(feature = "e1011", feature = "e1718"),
    all(feature = "e1011", feature = "e1011-server"),
    all(feature = "e1718", feature = "e1011-server")
))]
compile_error!("forza-hook: e1011, e1718 and e1011-server are three DLLs, one at a time");

mod dmi;
#[cfg_attr(not(feature = "e1011-server"), allow(dead_code))]
mod fixes;
#[cfg_attr(not(feature = "e1011"), allow(dead_code))]
mod logowin;
mod ports;
#[cfg_attr(not(feature = "e1011"), allow(dead_code))]
mod wholemsg;

#[cfg(target_arch = "x86")]
mod devices;
#[cfg(target_arch = "x86")]
mod log;
#[cfg(target_arch = "x86")]
mod net;
#[cfg(target_arch = "x86")]
mod patch;
#[cfg(target_arch = "x86")]
mod serial;
#[cfg(all(target_arch = "x86", feature = "e1011-server"))]
mod server;
#[cfg(all(target_arch = "x86", feature = "e1011"))]
mod whole;

#[cfg(target_arch = "x86")]
#[unsafe(no_mangle)]
extern "system" fn DllMain(
    inst: windows_sys::Win32::Foundation::HINSTANCE,
    reason: u32,
    _reserved: *mut std::ffi::c_void,
) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
    if reason == DLL_PROCESS_ATTACH {
        // SAFETY: our own module handle
        unsafe { DisableThreadLibraryCalls(inst) };
        log::open();
        devices::VERBOSE.store(
            std::env::var_os("MXHOOK_VERBOSE").is_some(),
            std::sync::atomic::Ordering::Relaxed,
        );
        if !install() {
            return 0; // the loader then refuses to start the game
        }
    }
    1
}

/// The hooks, in the order the log names them. False (2010-11 only): the DHCP calls may still reach the host.
#[cfg(target_arch = "x86")]
fn install() -> bool {
    use log::log;
    use std::sync::atomic::Ordering::Relaxed;
    #[cfg(feature = "e1011-server")]
    server::load_netguard();
    let dev = devices::install();
    let (release, renew, dhcp_unsure) = net::install_dhcp();
    #[cfg(feature = "e1011")]
    let create_process = logowin::install();

    // setup (crates/forza/src/setup/e1011.rs) finds the hook by these words in the file
    log!("==== mxhook attached (pid {}) ====", std::process::id());
    #[cfg(feature = "e1011-server")]
    server::install();
    let ringedge = std::env::var("MXHOOK_RINGEDGE").is_ok_and(|v| v == "1");
    devices::RINGEDGE.store(ringedge, Relaxed);
    serial::install();
    if ringedge {
        serial::hook_close();
    }
    log!(
        "RingEdge devices (MXHOOK_RINGEDGE): {}",
        if ringedge { "ON - mxsram" } else { "off" }
    );
    let (host, inet) = net::install_address();
    #[cfg(feature = "e1011")]
    whole::install();

    let b = u8::from;
    let [cfw, cfa, cfm, ofm, dio] = dev.map(b);
    #[cfg(feature = "e1011")]
    let cp = format!("  CreateProcessA={}", b(create_process));
    #[cfg(not(feature = "e1011"))]
    let cp = "";
    log!(
        "hooks installed: CreateFileW={cfw} CreateFileA={cfa} CreateFileMappingA={cfm} OpenFileMappingA={ofm} DeviceIoControl={dio}  IpRelease={} IpRenew={}{cp}  gethostbyname={} inet_addr={}",
        b(release),
        b(renew),
        b(host),
        b(inet)
    );
    // fail closed (2010-11): an iphlpapi import that could not be read or patched may still release the host's lease
    if cfg!(feature = "e1011") && dhcp_unsure {
        log!(
            "REFUSING TO RUN: the iphlpapi.dll imports could not be patched, so IpReleaseAddress/IpRenewAddress may reach the host"
        );
        return false;
    }
    true
}
