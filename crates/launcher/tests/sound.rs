//! The starting volumes through Windows' Core Audio, on a stand-in that is this test process: it opens a shared-mode
//! playback stream that plays only silence, so it has a session; the helper mutes it / sets its level and the test
//! reads both back.  A PC with no playback device cannot run this: the test then prints SKIPPED and checks nothing.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use launcher::helper::{Helper, Target, sound};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};

/// A started stream of silence in this process (the buffer is never written: it plays zeros), or None without a device.
fn silent_stream() -> Option<IAudioClient> {
    // SAFETY: Core Audio calls with valid arguments; the mix format comes from the client itself.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;
        let format = client.GetMixFormat().ok()?;
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, format, None)
            .ok()?;
        client.Start().ok()?;
        Some(client)
    }
}

#[test]
fn the_projector_starts_muted_and_seat_1_low() {
    let Some(_stream) = silent_stream() else {
        println!("SKIPPED: no playback device (or none that opens) on this PC - nothing checked");
        return;
    };
    let pid = std::process::id();
    let before = sound::read(pid).unwrap();
    assert!(!before.is_empty(), "the stand-in's stream has a session");
    let (level, muted) = before[0];

    // the helper, as the run starts it: the "projector" muted, the "seat" by level; both are this process here
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = said.clone();
    let mut helper = Helper::default();
    helper.start_volumes(
        vec![Target {
            name: "the stand-in",
            pid: Box::new(move || Some(pid)),
            mute: true,
        }],
        Helper::core_audio(),
        Arc::new(move |l: &str| log.lock().unwrap().push(l.to_owned())),
    );
    let t0 = Instant::now();
    while said.lock().unwrap().is_empty() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(helper);
    let now = sound::read(pid).unwrap();
    // put it back before judging: Windows remembers the level and mute per program
    sound::apply(pid, Some(level), Some(muted)).unwrap();
    let said = said.lock().unwrap().join("\n");
    assert!(said.contains("starts muted"), "{said}");
    assert!(
        now.iter().all(|&(v, m)| m && (v - sound::GAME_VOLUME).abs() < 0.005),
        "{now:?}"
    );
    assert_eq!(sound::read(pid).unwrap()[0], (level, muted), "restored");
    // one session by its identity (the pause's mute): an unknown id sets nothing, the session's own id sets it alone
    let ids = sound::mutes(pid).unwrap();
    assert_eq!(ids.len(), before.len(), "{ids:?}");
    let id = ids[0].0.clone();
    let one = std::slice::from_ref(&id);
    assert!(
        sound::set_mute(pid, &["no such session".to_owned()], !muted)
            .unwrap()
            .is_empty()
    );
    assert_eq!(sound::mutes(pid).unwrap()[0], (id.clone(), muted));
    assert_eq!(sound::set_mute(pid, one, !muted).unwrap(), one);
    assert_eq!(sound::mutes(pid).unwrap()[0], (id.clone(), !muted));
    sound::set_mute(pid, one, muted).unwrap();
    assert_eq!(sound::mutes(pid).unwrap()[0], (id, muted), "restored by id");
    // a process with no session is not an error: nothing set
    assert_eq!(sound::apply(u32::MAX - 1, Some(0.5), Some(true)).unwrap(), 0);
}
