//! Whole messages for control_Release (2026-10-11): the batch rules, without Windows calls (`whole_hook` uses them in
//! the server, the tests below check them on their own).
//!
//! What the cabinets send on TCP 20002 is a BATCH (from the disassembly): u32 record count, u32 total size INCLUDING
//! these 8 bytes, then count records of a 16-byte id (text, ended by a NUL unless all 16 are used), a u32 body size and
//! the body. Little-endian.
//!
//! The original server reads a batch whole when it arrives whole, as it always did on the arcade's LAN. Over the
//! internet TCP may hand it over in pieces, and the server's code for stitching pieces together is broken: it loses its
//! place and crashes (control_Release+0x129C2 / the memcpy at +0x12A3D), cutting every player. So the hook keeps the
//! pieces and gives the server only whole batches; the broken stitching code then never runs.
//!
//! Bytes that cannot be a batch never reach the server either: they are skipped up to the next place a batch can start.
//! Nobody is disconnected for them (a rule since 2026-10-11: no player is ever cut).
//!
//! 2017-18's control (2026-10-28) has the same batch (its sender, sub_4130F0/sub_413170: total = 8 + each record's 20 +
//! body) and the same stitching code (sub_413F30, recv 0x80000). One bug in it explains the crash at control+0x14838: when
//! a read ends inside a batch's total (bytes 4 to 8), sub_414490 stores the stitched total in the record count
//! (+0x38 at 0x414691, not +0x3C); control then reads the following bytes as records, takes a garbage body size, and a
//! negative one moves its read offset far outside its buffer. Whole batches never reach that code.

/// The server's own receive buffer (FUN_00412140: recv(..., 0x80000, 0)).
pub const MAX_BATCH: usize = 0x80000;
const HEAD: usize = 8;
/// A record's head: the 16-byte id and the u32 body size.
const REC: usize = 20;

fn u32_at(p: &[u8], at: usize) -> usize {
    u32::from_le_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]]) as usize
}

/// What the bytes at the start of `p` are.
#[derive(Debug, PartialEq, Eq)]
pub enum Batch {
    /// a whole batch of this many bytes
    Whole(usize),
    /// the start of one: wait for the rest
    Partial,
    /// no batch can start here
    Bad,
}

pub fn check(p: &[u8]) -> Batch {
    if p.len() < HEAD {
        return Batch::Partial;
    }
    let (count, total) = (u32_at(p, 0), u32_at(p, 4));
    if !(HEAD..=MAX_BATCH).contains(&total) || count > (total - HEAD) / REC {
        return Batch::Bad;
    }
    let mut off = HEAD;
    for _ in 0..count {
        if off + REC > p.len() {
            return Batch::Partial; // that record's head is not here yet
        }
        if off + REC > total {
            return Batch::Bad; // earlier bodies left no room for this record's head
        }
        // an id is text: no empty id, no control characters before its end
        let id = &p[off..off + 16];
        let len = id.iter().position(|&b| b == 0).unwrap_or(16);
        if len == 0 || id[..len].iter().any(|&b| b < 0x20) {
            return Batch::Bad;
        }
        let body = u32_at(p, off + 16);
        if body > total - off - REC {
            return Batch::Bad; // a body that runs past the batch
        }
        off += REC + body;
    }
    if off != total {
        return Batch::Bad; // the records do not fill the batch exactly
    }
    if p.len() >= total {
        Batch::Whole(total)
    } else {
        Batch::Partial
    }
}

/// 2017-18 control's bytes where [`crate::whole`] expects them (offsets in the program, ASLR moves its base): the push of
/// sub_413F30's recv(s, buf, 0x80000, 0) at 0x413FB7, and sub_414490's store of a stitched total in the count at 0x41468A.
#[cfg_attr(feature = "e1011", allow(dead_code))]
pub const CONTROL_1718: [(usize, &[u8]); 2] = [
    (
        0x13FB7,
        &[
            0x57, 0x68, 0x00, 0x00, 0x08, 0x00, 0x8D, 0x4C, 0x24, 0x30, 0x51, 0x50,
        ],
    ),
    (
        0x1468A,
        &[0x8B, 0x47, 0x38, 0x8B, 0x4C, 0x24, 0x18, 0x89, 0x0C, 0xB0],
    ),
];

/// The bytes in front of `p`'s first possible batch (skipped one at a time), and the size of the whole batches after
/// them that fit in `room` together (0: none yet).
pub fn whole(p: &[u8], room: usize) -> (usize, usize) {
    let skip = (0..p.len())
        .find(|&s| check(&p[s..]) != Batch::Bad)
        .unwrap_or(p.len());
    let mut used = 0;
    while let Batch::Whole(n) = check(&p[skip + used..]) {
        if used + n > room {
            break;
        }
        used += n;
    }
    (skip, used)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A batch of records (id, body size) with bodies of `b`.
    fn batch(recs: &[(&str, usize)], b: u8) -> Vec<u8> {
        let mut p = vec![0; HEAD];
        for &(id, size) in recs {
            let mut head = [0u8; REC];
            let n = id.len().min(16);
            head[..n].copy_from_slice(&id.as_bytes()[..n]);
            head[16..].copy_from_slice(&(size as u32).to_le_bytes());
            p.extend_from_slice(&head);
            p.extend(std::iter::repeat_n(b, size));
        }
        let total = p.len() as u32;
        p[..4].copy_from_slice(&(recs.len() as u32).to_le_bytes());
        p[4..8].copy_from_slice(&total.to_le_bytes());
        p
    }

    fn put(p: &mut [u8], at: usize, v: u32) {
        p[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn three() -> Vec<u8> {
        // bodies of zeros, like real ones often are
        batch(
            &[
                ("TO MATCH INI", 8524),
                ("KEEP_ALIVE", 0),
                ("SIXTEEN_CHAR_ID!", 4),
            ],
            0,
        )
    }

    #[test]
    fn whole_and_split_batches() {
        let a = three();
        let n1 = a.len();
        assert_eq!(n1, 8 + 20 * 3 + 8528);
        assert_eq!(check(&a), Batch::Whole(n1));
        // every place TCP can split it: wait for the rest, never a fault
        assert!((0..n1).all(|k| check(&a[..k]) == Batch::Partial));
        assert_eq!(
            whole(&a[..n1 - 1], MAX_BATCH),
            (0, 0),
            "one byte short: nothing yet, nothing skipped"
        );

        let mut two = a.clone();
        two.extend(batch(&[("TO MATCH INI", 284)], 0x41));
        let n2 = two.len() - n1;
        assert_eq!(
            whole(&two, MAX_BATCH),
            (0, n1 + n2),
            "two batches back to back: both"
        );
        assert_eq!(
            whole(&two, n1 + n2 - 1),
            (0, n1),
            "only what fits in the server's buffer"
        );
        assert_eq!(
            whole(&two[..n1 + 5], MAX_BATCH),
            (0, n1),
            "a whole one and the start of the next: the whole one"
        );

        let mut empty = a.clone();
        put(&mut empty, 0, 0);
        put(&mut empty, 4, 8);
        assert_eq!(
            check(&empty[..8]),
            Batch::Whole(8),
            "an empty batch is a batch"
        );
    }

    #[test]
    fn bad_batches() {
        let bad = |at: usize, v: u32, upto: Option<usize>| {
            let mut b = three();
            put(&mut b, at, v);
            check(&b[..upto.unwrap_or(b.len())])
        };
        assert_eq!(
            bad(4, 7, None),
            Batch::Bad,
            "total smaller than its own head"
        );
        assert_eq!(
            bad(4, MAX_BATCH as u32 + 1, None),
            Batch::Bad,
            "total bigger than the server's buffer"
        );
        assert_eq!(bad(0, 1000, None), Batch::Bad, "more records than fit");
        assert_eq!(
            bad(8 + 16, 8525, None),
            Batch::Bad,
            "a body size one too big"
        );
        assert_eq!(
            bad(8 + 16, 8523, None),
            Batch::Bad,
            "a body size one too small"
        );
        assert_eq!(
            bad(4, 0x9F54_06C1, Some(8)),
            Batch::Bad,
            "the wild length seen in the crash, from its first 8 bytes"
        );
        let mut b = three();
        b[8] = 0;
        assert_eq!(check(&b), Batch::Bad, "an empty id");
        let mut b = three();
        b[10] = 0x07;
        assert_eq!(check(&b), Batch::Bad, "a control character in an id");
    }

    #[test]
    fn the_2017_18_crash_cases() {
        let a = three();
        // the read that ends inside the total, where control's own stitching loses its count
        for k in 5..8 {
            assert_eq!(whole(&a[..k], MAX_BATCH), (0, 0), "split at {k}: held");
            assert_eq!(whole(&a, MAX_BATCH), (0, a.len()), "then given whole");
        }
        let mut neg = three();
        put(&mut neg, 8 + 16, 0xCE27_7B47); // the offset in the crash, as a body size: negative
        assert_eq!(check(&neg), Batch::Bad, "a negative body size");
        let mut tail = neg.clone();
        tail.extend_from_slice(&a);
        assert_eq!(
            whole(&tail, MAX_BATCH),
            (neg.len(), a.len()),
            "the bad batch skipped, the next one given"
        );
        let mut many = three();
        put(&mut many, 0, a.len() as u32); // a total read as the count
        assert_eq!(check(&many), Batch::Bad, "a count that cannot fit");
    }

    #[test]
    fn garbage_is_skipped() {
        let a = three();
        let mut buf = vec![0xEE; 37];
        buf.extend_from_slice(&a);
        assert_eq!(
            whole(&buf, MAX_BATCH),
            (37, a.len()),
            "garbage in front: skipped to the batch"
        );
        assert_eq!(
            whole(&buf[..37 + 100], MAX_BATCH),
            (37, 0),
            "garbage, then a batch not all here"
        );
    }
}
