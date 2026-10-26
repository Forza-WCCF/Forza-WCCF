//! The fake BIOS area `\\.\columba` reads from (2026-10-04).
//!
//! `\\.\columba` reads PHYSICAL memory (ioctl 0x9C406104, in: {address low, address high, unit, length}). The game
//! (amPlatform*, client FUN_00837e50 / FUN_00837f60) scans 0xF0000-0xFFFFF for the "_DMI_" anchor, reads the SMBIOS
//! table it points to, and keeps: type 0 texts 1-2 (BIOS version, date), type 1 text 0 (manufacturer), type 11 texts
//! 0-4 (OEM). Texts are counted from 0. amPlatformGetBoardType (FUN_00824070) wants OEM text 2 = "AAL" (RingEdge;
//! "AAM" = RingWide), manufacturer "NEC", and OEM text 4 not "AAL2" (else RingEdge 2); anything else -> unknown board
//! -> a NULL table -> the crash at ArcadeManager::initialize (FUN_007a5560). This area is built to those rules.

pub const BASE: u32 = 0xE0000;
pub const SIZE: u32 = 0x20000;
pub const TABLE: u32 = 0xE0000;
pub const ANCHOR: u32 = 0xF0010;

/// The area from BASE to BASE + SIZE: the SMBIOS table at TABLE, the "_DMI_" anchor at ANCHOR.
pub fn build() -> Vec<u8> {
    const T0: [u8; 18] = [
        0, 18, 0, 0, 1, 2, 0x00, 0xE8, 3, 0x0F, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    const S0: [&str; 3] = ["Phoenix Technologies, LTD", "6.00 PG", "06/20/2008"];
    const T1: [u8; 8] = [1, 8, 1, 0, 1, 2, 3, 4];
    const S1: [&str; 4] = ["NEC", "RingEdge", "1.00", "AAL0000000000"];
    const T11: [u8; 5] = [11, 5, 2, 0, 5];
    const S11: [&str; 5] = ["SEGA", "RINGEDGE", "AAL", "0", " "];
    const T127: [u8; 4] = [127, 4, 3, 0];
    let mut table = vec![];
    for (head, texts) in [(&T0[..], &S0[..]), (&T1, &S1), (&T11, &S11)] {
        table.extend_from_slice(head);
        for t in texts {
            table.extend_from_slice(t.as_bytes());
            table.push(0);
        }
        table.push(0); // the empty string that ends the set
    }
    table.extend_from_slice(&T127);
    table.extend_from_slice(&[0, 0]);

    let mut anchor = [0u8; 15];
    anchor[..5].copy_from_slice(b"_DMI_");
    anchor[6..8].copy_from_slice(&(table.len() as u16).to_le_bytes());
    anchor[8..12].copy_from_slice(&TABLE.to_le_bytes());
    anchor[12] = 4; // number of structures
    anchor[14] = 0x25; // SMBIOS 2.5
    anchor[5] = 0u8.wrapping_sub(anchor.iter().fold(0u8, |s, b| s.wrapping_add(*b)));

    let mut area = vec![0; SIZE as usize];
    let at = (TABLE - BASE) as usize;
    area[at..at + table.len()].copy_from_slice(&table);
    let at = (ANCHOR - BASE) as usize;
    area[at..at + anchor.len()].copy_from_slice(&anchor);
    area
}

/// `out.len()` bytes of physical memory from `addr`: the area where it covers them, zeros elsewhere.
pub fn read(area: &[u8], addr: u32, out: &mut [u8]) {
    for (i, b) in out.iter_mut().enumerate() {
        let p = addr.wrapping_add(i as u32);
        *b = if (BASE..BASE + SIZE).contains(&p) {
            area[(p - BASE) as usize]
        } else {
            0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_anchor_finds_a_ringedge_table() {
        let area = build();
        let mut anchor = [0u8; 15];
        read(&area, ANCHOR, &mut anchor);
        assert_eq!(&anchor[..5], b"_DMI_");
        assert_eq!(
            anchor.iter().fold(0u8, |s, b| s.wrapping_add(*b)),
            0,
            "the anchor's checksum"
        );
        let len = u16::from_le_bytes([anchor[6], anchor[7]]) as usize;
        let at = u32::from_le_bytes(anchor[8..12].try_into().unwrap());
        let mut table = vec![0; len];
        read(&area, at, &mut table);
        let text = String::from_utf8_lossy(&table);
        for want in ["NEC", "RINGEDGE", "\0AAL\0", "06/20/2008"] {
            assert!(text.contains(want), "{want:?} in the table");
        }
        assert!(
            table.ends_with(&[127, 4, 3, 0, 0, 0]),
            "the end-of-table structure"
        );
    }

    #[test]
    fn reads_outside_the_area_are_zeros() {
        let area = build();
        let mut out = [0xAAu8; 8];
        read(&area, BASE + SIZE - 4, &mut out);
        assert_eq!(&out[4..], &[0, 0, 0, 0]);
        read(&area, 0xFFFF_FFFC, &mut out);
        assert_eq!(out, [0; 8], "an address that wraps");
    }
}
