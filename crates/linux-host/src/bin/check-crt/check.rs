//! Check the exact logger label boundary using the CRT actually loaded by Wine.
use std::{
    ffi::{CString, c_void},
    sync::atomic::{AtomicUsize, Ordering},
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryA(name: *const i8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
}
static INVALID: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn rejected(_: *const u16, _: *const u16, _: *const u16, _: u32, _: usize) {
    INVALID.fetch_add(1, Ordering::Relaxed);
}
pub fn main() {
    // SAFETY: msvcr90's strcpy_s and _set_invalid_parameter_handler, called with their documented signatures
    unsafe {
        let dll = CString::new("msvcr90.dll").unwrap();
        let module = LoadLibraryA(dll.as_ptr());
        assert!(!module.is_null());
        let symbol = |name| {
            let name = CString::new(name).unwrap();
            let p = GetProcAddress(module, name.as_ptr());
            assert!(!p.is_null());
            p
        };
        let copy: unsafe extern "C" fn(*mut i8, usize, *const i8) -> i32 =
            std::mem::transmute(symbol("strcpy_s"));
        if std::env::args().nth(1).as_deref() == Some("fatal") {
            let text = CString::new("fixture-invalid-argument").unwrap();
            copy(std::ptr::null_mut(), 0, text.as_ptr());
            panic!("expected CRT termination");
        }
        let handler: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
            std::mem::transmute(symbol("_set_invalid_parameter_handler"));
        let old = handler(rejected as *mut c_void);
        let mut buffer = [0i8; 12];
        let long = CString::new("Match[Control]").unwrap();
        assert_ne!(copy(buffer.as_mut_ptr(), buffer.len(), long.as_ptr()), 0);
        assert_eq!(INVALID.load(Ordering::Relaxed), 1);
        for label in ["Match[Ctl]", "Match", "12345678901"] {
            let source = CString::new(label).unwrap();
            assert_eq!(copy(buffer.as_mut_ptr(), buffer.len(), source.as_ptr()), 0);
            assert_eq!(
                std::ffi::CStr::from_ptr(buffer.as_ptr()).to_bytes(),
                label.as_bytes()
            );
        }
        assert_eq!(INVALID.load(Ordering::Relaxed), 1);
        handler(old);
        println!(
            "PASS: original label rejected by char[12]; corrected, ordinary and maximum-length labels copy exactly"
        );
    }
}
