use super::*;

// ---------------------------------------------------------------- the programs' hook import (helpers/hook.py)
// NUL-ended and equally long, as `setup::rename_import` takes them (setup/e1011.rs has the same pair).
pub(super) const HOOK_DLL: &[u8] = b"wccfk.dll\0";
pub(super) const SYS_DLL: &[u8] = b"WINMM.dll\0";

/// The program with its one import of `frm` renamed to `to`; refused when it has `to` already or no `frm`.
pub(super) fn rename_import(d: &[u8], frm: &[u8], to: &[u8]) -> R<Vec<u8>> {
    let name = |n: &[u8]| String::from_utf8_lossy(&n[..n.len() - 1]).into_owned();
    crate::setup::rename_import(d, frm, to).map_err(|_| {
        Failed(if crate::setup::imports(d, to) {
            Line::key("text.fail_already_renamed").arg("name", name(to))
        } else {
            Line::key("text.fail_no_import").arg("name", name(frm))
        })
    })
}

fn is_exe(rel: &str) -> bool {
    rel.to_ascii_lowercase().ends_with(".exe")
}

pub(super) fn norm(d: Vec<u8>, rel: &str) -> R<Vec<u8>> {
    if is_exe(rel) && crate::setup::imports(&d, HOOK_DLL) {
        rename_import(&d, HOOK_DLL, SYS_DLL)
    } else {
        Ok(d)
    }
}

/// After a program is placed: its import renamed to the kit's hook, so it loads wccfk.dll. Idempotent.
pub(super) fn apply_hook(rel: &str, path: &Path) -> R<()> {
    if is_exe(rel) {
        let d = fs::read(path)?;
        if !crate::setup::imports(&d, HOOK_DLL) {
            put(path, &rename_import(&d, SYS_DLL, HOOK_DLL)?)?;
        }
    }
    Ok(())
}
