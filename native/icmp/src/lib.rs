//! The stand-in ICMP.DLL (bin\ICMP.DLL): the 2017-18 game programs LoadLibraryA("ICMP.DLL") and ping 192.168.46.254
//! with 5 s timeouts (about 45 s in the control, 20 s in each client). IcmpSendEcho returns 0 (no reply) at once and
//! writes nothing, the same result as the timeout. No packet leaves the PC.
#![cfg(target_arch = "x86")]

use std::ffi::c_void;
use windows_sys::Win32::Foundation::HANDLE;

#[unsafe(no_mangle)]
pub extern "system" fn IcmpCreateFile() -> HANDLE {
    1 as HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn IcmpCloseHandle(_: HANDLE) -> windows_sys::core::BOOL {
    1
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn IcmpSendEcho(
    _: HANDLE,
    _dest: u32,
    _data: *mut c_void,
    _size: u16,
    _options: *mut c_void,
    _reply: *mut c_void,
    _reply_size: u32,
    _timeout: u32,
) -> u32 {
    0
}
