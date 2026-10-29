//! IMPORT LINEUP's db.wccf.it share links: the one GET for a link's lineup text, through WinHTTP (the system's TLS and
//! proxy, no extra crate in the game). It sends nothing else: no cookies, no sign-in, no redirects followed.

use forza::cards::lineup::SHARE_HOST;
use forza_ui::panel::{self, FetchError};
use std::ffi::c_void;
use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR, w};

/// Each step's limit (name lookup, connect, send, each read), in ms; the panel gives up after `panel::FETCH_SECS`.
const STEP_MS: i32 = 5000;

/// A WinHTTP handle, closed on drop; a request handle reads as its answer's body.
struct H(*mut c_void);

impl H {
    fn new(h: *mut c_void) -> Result<H, FetchError> {
        if h.is_null() {
            Err(FetchError::Offline)
        } else {
            Ok(H(h))
        }
    }
}

impl Drop for H {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP gave, closed once
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

impl std::io::Read for H {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        let want = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: at most `want` bytes into `buf`, on a request whose answer came
        unsafe { WinHttpReadData(self.0, buf.as_mut_ptr().cast(), want, &mut n) }
            .map_err(std::io::Error::other)?;
        Ok(n as usize)
    }
}

/// `GET https://db.wccf.it{path}`: its body ([`panel::read_capped`]) on 200, `NotFound` on 404. Blocks: the share
/// fetch's own thread calls it.
pub fn get(path: String) -> Result<String, FetchError> {
    let off = |_| FetchError::Offline;
    // SAFETY: handles checked non-null, each closed after the ones opened from it (drop order); buffers outlive the calls
    unsafe {
        let s = H::new(WinHttpOpen(
            w!("Forza-WCCF"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(s.0, STEP_MS, STEP_MS, STEP_MS, STEP_MS).map_err(off)?;
        let c = H::new(WinHttpConnect(s.0, &HSTRING::from(SHARE_HOST), 443, 0))?;
        let mut r = H::new(WinHttpOpenRequest(
            c.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
        WinHttpSetOption(Some(r.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never)).map_err(off)?;
        let off_features = (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION).to_ne_bytes();
        WinHttpSetOption(
            Some(r.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&off_features),
        )
        .map_err(off)?;
        WinHttpSendRequest(r.0, None, None, 0, 0, 0).map_err(off)?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(off)?;
        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            r.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(off)?;
        match status {
            200 => panel::read_capped(&mut r),
            404 => Err(FetchError::NotFound),
            500.. => Err(FetchError::Offline),
            _ => Err(FetchError::NotLineup),
        }
    }
}
