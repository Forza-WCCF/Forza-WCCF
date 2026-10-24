//! Not shipped. test_wccfnet.exe <path to wccfnet.dll>: loads the guard from its own folder and calls its four stubs
//! with NULL (they ignore the argument). It never imports or loads IPHLPAPI itself, and calls a stub only once its
//! address is proven to lie inside the guard's image, so a wrong export can never reach the real IP helper.
#[path = "../check.rs"]
mod check;
use check::{check, done, load, owner};
use std::process::ExitCode;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

fn iphlpapi_loaded() -> bool {
    // SAFETY: a NUL-terminated name
    !unsafe { GetModuleHandleA(c"iphlpapi.dll".as_ptr().cast()) }.is_null()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return ExitCode::from(2);
    }
    check(
        !iphlpapi_loaded(),
        "IPHLPAPI is not loaded before the guard",
    );
    let g = load(&args[1]);
    if !check(!g.is_null(), "wccfnet.dll loads") {
        return ExitCode::from(1);
    }
    let log = std::path::Path::new(&args[1]).with_file_name("wccfnet.log");
    let _ = std::fs::remove_file(&log);
    let names = [
        c"IpReleaseAddress",
        c"IpRenewAddress",
        c"CreateIpForwardEntry",
        c"SetIpForwardEntry",
    ];
    for (i, name) in names.iter().enumerate() {
        let shown = name.to_string_lossy();
        // SAFETY: a NUL-terminated name in a module this program loaded
        let p = unsafe { GetProcAddress(g, name.as_ptr().cast()) };
        let inside = p.is_some_and(|p| owner(p as usize) == g);
        if !check(inside, &format!("{shown} resolves inside wccfnet.dll")) {
            continue;
        }
        // SAFETY: a stub of the guard (inside its image, checked above): DWORD WINAPI f(void *), the argument ignored
        let r = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, check::IpCall>(
                p.expect("checked above"),
            )(std::ptr::null_mut())
        };
        check(r == 0, &format!("{shown}(NULL) returns NO_ERROR"));
        let lines = std::fs::read(&log).map_or(0, |b| b.iter().filter(|&&c| c == b'\n').count());
        check(
            lines == i + 1,
            &format!("wccfnet.log has {} line(s)", i + 1),
        );
    }
    check(!iphlpapi_loaded(), "IPHLPAPI is still not loaded");
    done("wccfnet test")
}
