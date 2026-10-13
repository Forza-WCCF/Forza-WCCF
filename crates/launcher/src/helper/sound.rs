//! Windows' Core Audio, for the game's starting volumes (sound.py's place): every program that plays sound has a
//! session on its playback device, with the volume and mute the Volume Mixer shows.  The only module with the `windows`
//! crate (the one maintained binding that has these COM interfaces).
//!
//! Path: IMMDeviceEnumerator -> each active playback device -> IAudioSessionManager2 -> its sessions ->
//! IAudioSessionControl2 (the process id) and ISimpleAudioVolume (level and mute).  A session exists only once the
//! process has opened its sound, so a caller retries until `apply` finds one.

use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::core::{Interface, Result};

/// Seat 1 starts at 4%: "very low", as the player's own Volume Mixer had it (2026-10-06); the projector, muted, is at
/// the same level, so unmuting it in the Volume Mixer is never a blast.
pub const GAME_VOLUME: f32 = 0.04;

/// COM on this thread for as long as the guard lives (a thread that already has it in another mode keeps its own).
struct Com(bool);

impl Com {
    fn start() -> Self {
        // SAFETY: plain COM initialisation of the calling thread; undone in `drop` only when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: pairs the successful CoInitializeEx of `start` on this same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// `f(process id, its volume control)` for every sound session on every active playback device.  A device or session
/// that cannot be read is skipped, never the whole list; Err only when there is no device enumerator at all.
fn each_session(mut f: impl FnMut(u32, &ISimpleAudioVolume)) -> Result<()> {
    let _com = Com::start();
    // SAFETY: Core Audio's documented object creation and calls; every interface is released when dropped.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..devices.GetCount()? {
            let Ok(sessions) = devices
                .Item(i)
                .and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None))
                .and_then(|m| m.GetSessionEnumerator())
            else {
                continue;
            };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(ctl) = sessions.GetSession(j) else { continue };
                let pid = ctl.cast::<IAudioSessionControl2>().and_then(|c| c.GetProcessId());
                if let (Ok(pid), Ok(vol)) = (pid, ctl.cast::<ISimpleAudioVolume>()) {
                    f(pid, &vol);
                }
            }
        }
    }
    Ok(())
}

/// Every sound session of process `pid`: its volume (0-1) and / or mute set, as the Volume Mixer sets them.  How many
/// sessions were set (0: the process has no sound open yet).
pub fn apply(pid: u32, volume: Option<f32>, mute: Option<bool>) -> Result<usize> {
    let mut done = 0;
    each_session(|p, vol| {
        if p != pid {
            return;
        }
        // SAFETY: `vol` is a live ISimpleAudioVolume; a null event context is allowed.
        let set = unsafe {
            volume
                .map_or(Ok(()), |v| vol.SetMasterVolume(v.clamp(0.0, 1.0), std::ptr::null()))
                .and_then(|()| mute.map_or(Ok(()), |m| vol.SetMute(m, std::ptr::null())))
        };
        done += usize::from(set.is_ok());
    })?;
    Ok(done)
}

/// (volume, muted) of every sound session of process `pid`; read only.
pub fn read(pid: u32) -> Result<Vec<(f32, bool)>> {
    let mut out = Vec::new();
    each_session(|p, vol| {
        if p == pid {
            // SAFETY: `vol` is a live ISimpleAudioVolume.
            if let (Ok(level), Ok(muted)) = unsafe { (vol.GetMasterVolume(), vol.GetMute()) } {
                out.push((level, muted.as_bool()));
            }
        }
    })?;
    Ok(out)
}
