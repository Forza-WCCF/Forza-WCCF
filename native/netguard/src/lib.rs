//! netguard.dll: the 2010-11 server hook under Wine (native/2010-11/server-hook, C) loads it as its second guard. The
//! four IP helper calls that release the host's DHCP lease or write its routes (imported by control_Release.exe) are
//! pointed at stubs that touch nothing, in the import table of the program that loads it. Not in either kit.
#![cfg(target_arch = "x86")]

use forza_native::{By, imports, redirect};
use std::ffi::{CStr, c_void};
use std::io::Write;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// netguard.log beside the program, opened on the first line.
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn log(line: &str) {
    let mut f = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if f.is_none() {
        let exe = std::env::current_exe().unwrap_or_default();
        *f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(exe.with_file_name("netguard.log"))
            .ok();
    }
    if let Some(f) = f.as_mut() {
        let _ = writeln!(f, "netguard: {line}");
    }
}

fn blocked(name: &str) -> u32 {
    log(&format!("blocked {name}"));
    0 // NO_ERROR
}

extern "system" fn stub_release(_: *mut c_void) -> u32 {
    blocked("IpReleaseAddress")
}
extern "system" fn stub_renew(_: *mut c_void) -> u32 {
    blocked("IpRenewAddress")
}
extern "system" fn stub_create(_: *mut c_void) -> u32 {
    blocked("CreateIpForwardEntry")
}
extern "system" fn stub_set(_: *mut c_void) -> u32 {
    blocked("SetIpForwardEntry")
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(
    inst: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> windows_sys::core::BOOL {
    if reason != DLL_PROCESS_ATTACH {
        return 1;
    }
    // SAFETY: our own module handle
    unsafe { DisableThreadLibraryCalls(inst) };
    type Stub = extern "system" fn(*mut c_void) -> u32;
    let stubs: [(&CStr, Stub); 4] = [
        (c"IpReleaseAddress", stub_release),
        (c"IpRenewAddress", stub_renew),
        (c"CreateIpForwardEntry", stub_create),
        (c"SetIpForwardEntry", stub_set),
    ];
    // SAFETY: the process's main module, loaded for its whole life
    for d in unsafe { imports(GetModuleHandleW(std::ptr::null()) as *const u8) } {
        if !d.name().to_bytes().eq_ignore_ascii_case(b"IPHLPAPI.DLL") {
            continue;
        }
        // no name table: the slots hold resolved addresses, not names; never read them as names
        if !d.has_names() {
            log(&format!(
                "{} has no name table, not patched",
                d.name().to_string_lossy()
            ));
            continue;
        }
        for (by, slot) in d.entries() {
            if let Some((name, stub)) = stubs.iter().find(|(n, _)| by == By::Name(n)) {
                // SAFETY: an import slot of the loaded program; each stub has its import's signature
                if unsafe { redirect(slot, *stub as usize) }.is_none() {
                    // fail closed: the slot still reaches the IP helper, so the load fails and the server hook
                    // refuses to run without its second guard
                    log(&format!(
                        "{} could not be patched: NOT loaded",
                        name.to_string_lossy()
                    ));
                    return 0;
                }
                log(&format!("blocked {}", name.to_string_lossy()));
            }
        }
    }
    1
}
