//! The game's I/O board outputs (JVS command 0x32), shared from the launcher to the panel inside the game: a named
//! section of one word per seat, `Local\ForzaWCCF.jvs_out.<seat>`. The panel lights the tactics buttons from it.

/// The tactics lamps in the 2017-18 outputs' first byte: CENTRAL 0x80, COUNTER 0x40, L-SIDE 0x20, R-SIDE 0x10, PRESS 0x08
/// (the order of `PanelState::tactics`). SHOOT, KEEPER and KEY PLAYER bits are unconfirmed and not read.
pub fn tactics(outs: &[u8]) -> [bool; 5] {
    let b0 = outs.first().copied().unwrap_or(0);
    [0x80, 0x40, 0x20, 0x10, 0x08].map(|m| b0 & m != 0)
}

#[cfg(windows)]
pub use map::Map;

#[cfg(windows)]
mod map {
    use std::sync::atomic::{AtomicU32, Ordering::*};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };

    /// Set when the word holds outputs; the three bytes sit in bits 0-23.
    const VALID: u32 = 1 << 31;

    /// The seat's section, mapped. Both sides create it (the second gets the first's).
    pub struct Map {
        handle: HANDLE,
        word: *const AtomicU32,
    }

    // SAFETY: the view is shared memory touched only through one atomic word
    unsafe impl Send for Map {}
    // SAFETY: as above
    unsafe impl Sync for Map {}

    impl Map {
        pub fn create(seat: u32) -> Option<Map> {
            let name: Vec<u16> = format!("Local\\ForzaWCCF.jvs_out.{seat}\0")
                .encode_utf16()
                .collect();
            // SAFETY: a 4-byte page-file section and its view, closed in Drop
            unsafe {
                let handle = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    0,
                    4,
                    name.as_ptr(),
                );
                if handle.is_null() {
                    return None;
                }
                let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 4);
                if view.Value.is_null() {
                    CloseHandle(handle);
                    return None;
                }
                Some(Map {
                    handle,
                    word: view.Value.cast(),
                })
            }
        }

        fn word(&self) -> &AtomicU32 {
            // SAFETY: the view is 4 bytes, page-aligned, until Drop
            unsafe { &*self.word }
        }

        /// Publishes the game's outputs (the first three bytes).
        pub fn set(&self, outs: &[u8]) {
            let b = |i| outs.get(i).copied().unwrap_or(0) as u32;
            self.word()
                .store(VALID | b(0) | b(1) << 8 | b(2) << 16, Release);
        }

        /// The latest outputs, or `None` before the game's first command 0x32.
        pub fn get(&self) -> Option<[u8; 3]> {
            let w = self.word().load(Acquire);
            (w & VALID != 0).then_some([w as u8, (w >> 8) as u8, (w >> 16) as u8])
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            // SAFETY: the view and handle this Map made
            unsafe {
                UnmapViewOfFile(
                    windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.word as *mut _,
                    },
                );
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamps_from_outputs() {
        assert_eq!(tactics(&[0xB8, 0, 0x60]), [true, false, true, true, true]);
        assert_eq!(tactics(&[]), [false; 5]);
    }

    #[cfg(windows)]
    #[test]
    fn map_round_trip() {
        let a = Map::create(901).unwrap();
        assert_eq!(a.get(), None);
        a.set(&[0xB8, 0, 0x60]);
        assert_eq!(Map::create(901).unwrap().get(), Some([0xB8, 0, 0x60]));
    }
}
