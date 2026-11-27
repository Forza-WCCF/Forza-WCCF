//! A yes/no question ("Stop anyway?") in a MessageBoxW whose Yes and No read in the Launcher's language: MessageBoxW
//! labels them in the Windows one, so a hook on the asking thread relabels the box as it opens (#438).

use std::cell::Cell;
use std::ptr;

use forza_ui::{Lang, text as ui_text};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWPRETSTRUCT, CallNextHookEx, GetClassNameW, GetDlgItem, HC_ACTION, HHOOK, IDNO, IDYES, MB_ICONQUESTION,
    MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, SetDlgItemTextW, SetWindowsHookExW, UnhookWindowsHookEx, WH_CALLWNDPROCRET,
    WM_INITDIALOG,
};

use super::{message, wide};

thread_local! {
    /// The Yes and No labels for the next yes/no dialog this thread opens; the first one takes them.
    static LABELS: Cell<Option<[Vec<u16>; 2]>> = const { Cell::new(None) };
}

/// A yes/no question in front of the game (no owner: any thread may ask); true on yes.
pub(crate) fn ask_box(question: &str, lang: Lang) -> bool {
    let _relabel = Relabel::arm(lang);
    message(question, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND) == IDYES
}

/// The labels armed and the hook set on this thread until dropped (also on unwind).
struct Relabel(HHOOK);

impl Relabel {
    fn arm(lang: Lang) -> Self {
        LABELS.set(Some(
            ["launcher.ask_yes", "launcher.ask_no"].map(|k| wide(&ui_text(lang, k, &[]))),
        ));
        // SAFETY: a hook of this thread only, removed on drop; `relabel` reads this thread's LABELS.
        Self(unsafe { SetWindowsHookExW(WH_CALLWNDPROCRET, Some(relabel), ptr::null_mut(), GetCurrentThreadId()) })
    }
}

impl Drop for Relabel {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the hook set in `arm`, on this thread.
            unsafe { UnhookWindowsHookEx(self.0) };
        }
        LABELS.set(None);
    }
}

/// [`Relabel`]'s hook: when a dialog with Yes and No has handled WM_INITDIALOG (shown or not, active or not: the game
/// may hold the foreground), its buttons take the armed labels, once.
unsafe extern "system" fn relabel(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: a WH_CALLWNDPROCRET hook's `l` points to the call's CWPRETSTRUCT.
        let m = unsafe { &*(l as *const CWPRETSTRUCT) };
        if m.message == WM_INITDIALOG
            && is_yes_no_box(m.hwnd)
            && let Some([yes, no]) = LABELS.take()
        {
            // SAFETY: the dialog's own buttons; NUL-terminated labels that outlive the calls (taken: no borrow held).
            unsafe {
                SetDlgItemTextW(m.hwnd, IDYES, yes.as_ptr());
                SetDlgItemTextW(m.hwnd, IDNO, no.as_ptr());
            }
        }
    }
    // SAFETY: the next hook gets the call as it came.
    unsafe { CallNextHookEx(ptr::null_mut(), code, w, l) }
}

/// A dialog (class #32770) with a Yes and a No button.
fn is_yes_no_box(w: HWND) -> bool {
    let mut class = [0u16; 8];
    // SAFETY: a buffer of its stated length.
    let n = unsafe { GetClassNameW(w, class.as_mut_ptr(), 8) };
    let n = usize::try_from(n).unwrap_or(0);
    // SAFETY: plain lookups of `w`'s children.
    String::from_utf16_lossy(&class[..n]) == "#32770"
        && [IDYES, IDNO].iter().all(|&id| !unsafe { GetDlgItem(w, id) }.is_null())
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetDlgItemTextW, KillTimer, MessageBoxW, PostMessageW, SendMessageW, SetTimer,
        WM_COMMAND, WS_CHILD,
    };

    use super::*;

    /// A hidden window of `class`, with Yes and No buttons labelled "x" when `buttons`; destroyed on drop.
    struct Hidden(HWND);

    impl Hidden {
        fn new(class: &str, buttons: bool) -> Self {
            let (class, button, x) = (wide(class), wide("BUTTON"), wide("x"));
            // SAFETY: NUL-terminated strings; no parent, never shown (no WS_VISIBLE).
            let w = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    x.as_ptr(),
                    0,
                    0,
                    0,
                    200,
                    100,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            assert!(!w.is_null(), "a hidden {} window", String::from_utf16_lossy(&class));
            for id in [IDYES, IDNO].into_iter().filter(|_| buttons) {
                // SAFETY: a child of `w` with the control id `id` (its "menu").
                let b = unsafe {
                    CreateWindowExW(
                        0,
                        button.as_ptr(),
                        x.as_ptr(),
                        WS_CHILD,
                        0,
                        0,
                        50,
                        20,
                        w,
                        id as _,
                        ptr::null_mut(),
                        ptr::null(),
                    )
                };
                assert!(!b.is_null());
            }
            Self(w)
        }

        /// The window gets WM_INITDIALOG, as a dialog does when it is made; then its Yes and No labels.
        fn init(&self) -> [String; 2] {
            // SAFETY: a window of this thread.
            unsafe { SendMessageW(self.0, WM_INITDIALOG, 0, 0) };
            [IDYES, IDNO].map(|id| label(self.0, id))
        }
    }

    impl Drop for Hidden {
        fn drop(&mut self) {
            // SAFETY: the window made in `new`, on this thread.
            unsafe { DestroyWindow(self.0) };
        }
    }

    fn label(dlg: HWND, id: i32) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: a buffer of its stated length.
        let n = unsafe { GetDlgItemTextW(dlg, id, buf.as_mut_ptr(), 64) };
        String::from_utf16_lossy(&buf[..n as usize])
    }

    fn labels(lang: Lang) -> [String; 2] {
        ["launcher.ask_yes", "launcher.ask_no"].map(|k| ui_text(lang, k, &[]))
    }

    /// "Stop anyway?" reads Yes and No in the Launcher's language (#438), on the first yes/no dialog only: not on
    /// another window of the thread, nor on a later dialog. No box is shown (no focus taken).
    #[test]
    fn the_stop_question_answers_in_the_launchers_language() {
        let x = || ["x".to_owned(), "x".to_owned()];
        for lang in [Lang::It, Lang::Ja] {
            let _relabel = Relabel::arm(lang);
            assert_eq!(
                Hidden::new("#32770", false).init(),
                [String::new(), String::new()],
                "no Yes and No"
            );
            assert_eq!(Hidden::new("BUTTON", true).init(), x(), "not a dialog");
            assert_eq!(Hidden::new("#32770", true).init(), labels(lang), "{lang:?}");
            assert_eq!(Hidden::new("#32770", true).init(), x(), "once");
        }
        assert_eq!(Hidden::new("#32770", true).init(), x(), "unhooked on drop");
    }

    /// The real box, inactive (shown by a background process, no foreground asked): its labels as it is up. Opens a
    /// window for a moment: `cargo nextest run -p launcher --run-ignored only real_box`.
    #[test]
    #[ignore = "opens a message box"]
    fn the_real_box_is_relabelled() {
        thread_local!(static SEEN: Cell<Option<[String; 2]>> = const { Cell::new(None) });
        unsafe extern "system" fn answer(_: HWND, _: u32, id: usize, _: u32) {
            let title = wide("wccf-ask-test");
            // SAFETY: NUL-terminated strings; the box is this thread's, its answer ends MessageBoxW below.
            unsafe {
                let dlg = windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(ptr::null(), title.as_ptr());
                if !dlg.is_null() {
                    SEEN.set(Some([IDYES, IDNO].map(|i| label(dlg, i))));
                    PostMessageW(dlg, WM_COMMAND, IDNO as usize, 0);
                    KillTimer(ptr::null_mut(), id);
                }
            }
        }
        let _relabel = Relabel::arm(Lang::Ja);
        let (text, title) = (wide("Stop anyway?"), wide("wccf-ask-test"));
        // SAFETY: a thread timer that answers the box from inside its own loop; NUL-terminated strings.
        let rc = unsafe {
            SetTimer(ptr::null_mut(), 0, 200, Some(answer));
            MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO)
        };
        assert_eq!(rc, IDNO);
        assert_eq!(SEEN.take(), Some(labels(Lang::Ja)));
    }
}
