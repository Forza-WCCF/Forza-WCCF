//! inject.exe <pid> <full path of forza_panel.dll>: loads the panel into a running 32-bit game process (2010-11: seat
//! 1's client_Release.exe, and the projector's). 32-bit itself, so its kernel32 LoadLibraryW address is the game's.
use std::process::ExitCode;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    WaitForSingleObject,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        println!("usage: inject <pid> <dllpath>");
        return ExitCode::from(2);
    }
    let pid: u32 = args[1].trim().parse().unwrap_or(0);
    let path: Vec<u16> = args[2].encode_utf16().chain([0]).collect();
    let bytes = path.len() * 2;
    // SAFETY: the game process opened here; the path is copied into memory allocated in it, and LoadLibraryW (the same
    // address in every 32-bit process of this session) runs there on it
    unsafe {
        let access = PROCESS_CREATE_THREAD
            | PROCESS_VM_OPERATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_READ
            | PROCESS_QUERY_INFORMATION;
        let p = OpenProcess(access, 0, pid);
        if p.is_null() {
            println!("OpenProcess({pid}) failed err={}", GetLastError());
            return ExitCode::from(1);
        }
        let fail = |what: &str| {
            println!("{what} failed err={}", GetLastError());
            CloseHandle(p);
            ExitCode::from(1)
        };
        let rem = VirtualAllocEx(
            p,
            std::ptr::null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if rem.is_null() {
            return fail("VirtualAllocEx");
        }
        let mut put = 0;
        if WriteProcessMemory(p, rem, path.as_ptr().cast(), bytes, &mut put) == 0 {
            return fail("WriteProcessMemory");
        }
        let ll = GetProcAddress(
            GetModuleHandleA(c"kernel32.dll".as_ptr().cast()),
            c"LoadLibraryW".as_ptr().cast(),
        )
        .expect("kernel32 exports LoadLibraryW");
        let start = std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        >(ll);
        let t = CreateRemoteThread(
            p,
            std::ptr::null(),
            0,
            Some(start),
            rem,
            0,
            std::ptr::null_mut(),
        );
        if t.is_null() {
            return fail("CreateRemoteThread");
        }
        WaitForSingleObject(t, 6000);
        let mut ec = 0;
        GetExitCodeThread(t, &mut ec);
        let failed = if ec == 0 { " (0 = FAILED to load)" } else { "" };
        println!("injected into pid {pid}; LoadLibraryW returned module 0x{ec:08X}{failed}");
        CloseHandle(t);
        CloseHandle(p);
        ExitCode::from(u8::from(ec == 0))
    }
}
