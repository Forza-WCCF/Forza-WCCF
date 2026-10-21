//! launcher::procs on this PC: nothing live, no game.  Stand-ins are copies of cmd.exe and this test program.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs, mem, ptr};

use launcher::procs::{
    game_processes, image, listening_ports, normcase, pipes, prefers_system32, processes, same_form,
};
use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, PROCESS_INFORMATION, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

#[test]
fn this_process_is_listed_with_its_path() {
    let me = processes()
        .into_iter()
        .find(|p| p.pid == std::process::id())
        .expect("this process");
    let exe = env::current_exe().unwrap();
    assert_eq!(normcase(&me.path), normcase(&exe));
    assert!(me.name.eq_ignore_ascii_case(exe.file_name().unwrap().to_str().unwrap()));
}

/// Port of legacy/2010-11/tests/test_junction_paths.py: a program started through a junction is found by the path
/// checks.  Windows reports it under the junction's target, so same_form of the junction path must equal the path
/// processes() reads (2026-10-12: a kit started through its first-run link could not stop its own helpers).
#[test]
fn a_program_started_through_a_junction_is_found() {
    let top = tempfile::tempdir().unwrap(); // %TEMP%: a local NTFS drive, where junctions work
    let (real, link) = (top.path().join("real"), top.path().join("link"));
    fs::create_dir(&real).unwrap();
    let system = env::var_os("SystemRoot").unwrap();
    fs::copy(
        Path::new(&system).join("System32").join("cmd.exe"),
        real.join("cmd.exe"),
    )
    .unwrap();
    junction::create(&real, &link).unwrap();
    let mut child = Command::new(link.join("cmd.exe"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let seen = processes().into_iter().find(|p| p.pid == child.id()).map(|p| p.path);
    let direct = image(child.id());
    let want = same_form(&link.join("cmd.exe"));
    let _ = child.kill();
    let _ = child.wait();
    fs::remove_dir(&link).unwrap(); // the junction only, never its target's files
    let seen = seen.expect("the stand-in is listed");
    assert!(!seen.as_os_str().is_empty(), "the stand-in's path could not be read");
    assert_eq!(direct.as_ref(), Some(&seen));
    assert_eq!(normcase(&seen), want, "{seen:?} through {link:?}");
    assert_ne!(
        normcase(&link.join("cmd.exe")),
        normcase(&seen),
        "Windows names the target, not the junction"
    );
}

#[test]
fn same_form_keeps_a_missing_tail_and_folds_case() {
    let top = tempfile::tempdir().unwrap();
    let real = same_form(top.path());
    assert!(!real.starts_with(r"\\?\"), "{real}");
    assert_eq!(
        same_form(&top.path().join("Not").join("THERE.exe")),
        format!(r"{real}\not\there.exe")
    );
    assert_eq!(
        same_form(&top.path().join("a").join("..").join("B")),
        format!(r"{real}\b")
    );
}

/// A folder named in 8.3 short form is put in the long form Windows reports a process's path in (2026-10-21: the CI
/// runner's %TEMP% is C:\Users\RUNNER~1 and a program in it was reported under C:\Users\runneradmin).
#[test]
fn same_form_expands_short_names() {
    let top = tempfile::tempdir().unwrap();
    let long = top.path().join("A Long Folder Name");
    fs::create_dir(&long).unwrap();
    let wide: Vec<u16> = long.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = [0u16; 1024];
    // SAFETY: wide ends in a 0; the buffer and its length go together.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    assert!(n > 0 && n < buf.len(), "GetShortPathNameW of {long:?}");
    let short = std::path::PathBuf::from(String::from_utf16(&buf[..n]).unwrap());
    if short.file_name() == long.file_name() {
        eprintln!("no 8.3 names on this volume ({short:?}): nothing to expand");
        return;
    }
    // no part of this path has a ~ in its long name: %TEMP%, tempfile's .tmpXXXXXX and the folder above
    let expanded = same_form(&short);
    assert!(
        !expanded.contains('~') && expanded.ends_with(r"\a long folder name"),
        "{expanded}"
    );
    assert_eq!(expanded, same_form(&long));
    assert_eq!(same_form(&short.join("Game.exe")), same_form(&long.join("Game.exe")));
    // what the runner showed: a junction made from a short-named %TEMP%, its target in short form too
    fs::create_dir(long.join("real")).unwrap();
    fs::write(long.join("real").join("game.exe"), b"").unwrap();
    junction::create(short.join("real"), short.join("link")).unwrap();
    let through = same_form(&short.join("link").join("game.exe"));
    fs::remove_dir(long.join("link")).unwrap(); // the junction only
    assert_eq!(through, same_form(&long.join("real").join("game.exe")));
}

#[test]
fn no_game_runs_from_a_scratch_folder() {
    let top = tempfile::tempdir().unwrap();
    assert!(game_processes(&top.path().join("game")).is_empty());
}

#[test]
fn a_listening_port_and_the_pipes_are_seen() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(listening_ports().contains(&port));
    drop(l);
    assert!(!pipes().is_empty(), "Windows always has named pipes");
}

/// Run by `prefer_system32_is_seen` in a copy of this test program started with the policy on.
#[test]
fn probe_prefer_system32() {
    if env::var_os("FORZA_PROBE_PREFER").is_some() {
        std::process::exit(if prefers_system32() { 7 } else { 8 });
    }
}

/// A program started with "Prefer System32 images" on (as wslhost.exe starts them) sees it.
#[test]
fn prefer_system32_is_seen() {
    // PROCESS_CREATION_MITIGATION_POLICY_IMAGE_LOAD_PREFER_SYSTEM32_ALWAYS_ON (winnt.h: 1 << 60)
    let policy: u64 = 1 << 60;
    let exe = env::current_exe().unwrap();
    let line = format!("\"{}\" probe_prefer_system32 --exact --test-threads 1", exe.display());
    let mut line: Vec<u16> = std::ffi::OsStr::new(&line).encode_wide().chain([0]).collect();
    let mut block: Vec<u16> = env::vars_os()
        .chain([("FORZA_PROBE_PREFER".into(), "1".into())])
        .flat_map(|(k, v)| {
            let mut kv = k;
            kv.push("=");
            kv.push(v);
            kv.encode_wide().chain([0]).collect::<Vec<u16>>()
        })
        .chain([0])
        .collect();
    // SAFETY: the attribute list lives in `list` for the whole call; every pointer outlives CreateProcessW; the
    // handles it returns are closed once.
    unsafe {
        let mut size = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        let mut list = vec![0u8; size];
        let attrs: LPPROC_THREAD_ATTRIBUTE_LIST = list.as_mut_ptr().cast();
        assert!(InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) != 0);
        let set = UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
            (&raw const policy).cast(),
            size_of::<u64>(),
            ptr::null_mut(),
            ptr::null(),
        );
        assert!(set != 0);
        let mut si: STARTUPINFOEXW = mem::zeroed();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = mem::zeroed();
        let made = CreateProcessW(
            ptr::null(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            FALSE,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            block.as_mut_ptr().cast(),
            ptr::null(),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        assert!(made != 0, "{}", std::io::Error::last_os_error());
        WaitForSingleObject(pi.hProcess, INFINITE);
        let mut code = 0;
        GetExitCodeProcess(pi.hProcess, &mut code);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        assert_eq!(
            code, 7,
            "a program started with Prefer System32 images on must see it (8 = not seen)"
        );
    }
}
