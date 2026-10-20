//! 2017-18 manager titles: `CoachtitleTableRecord.bin`, read only by the client (a generic `enumdata` table whose
//! 52-byte records hold the strings by pointer, so a string may change length).
//!
//! Layout: u32 count, then per title u32 id, u32 flag, 3 x (u32 length, cp932 bytes, no end byte): key, title,
//! requirement, then 32 bytes of numbers. Only the title and the requirement change; sheet `coach_titles.tsv` keys
//! are `title_NNNN` and `requirement_NNNN`, NNNN the record's id (the ids run 0, 1, 2...; the original keys are
//! romanised Japanese and one holds the publisher's name).

use super::{Built, Ctx, Failed, Lang, R, cell, fail, sha};

pub(super) const TITLES: &str = "prog_data/enumdata/CoachtitleTableRecord.bin";
pub(super) const TITLES_SHA: &str =
    "8987C49FC689972B9B9F87F9B9FB940F264AD09956BD3ACCEDAF06FD7E30D386";
/// The longest original title and requirement in bytes: what the title screen is known to show.
const ROOM: [usize; 2] = [24, 62];
const PART: [&str; 2] = ["title", "requirement"];

/// id + flag, [key, title, requirement], the numbers after them.
type Rec = ([u8; 8], [Vec<u8>; 3], [u8; 32]);

fn read(d: &[u8]) -> R<Vec<Rec>> {
    let short = || Failed("CoachtitleTableRecord.bin is cut short".into());
    let take = |p: &mut usize, n: usize| -> R<&[u8]> {
        let b = d.get(*p..*p + n).ok_or_else(short)?;
        *p += n;
        Ok(b)
    };
    let mut p = 0;
    let n = u32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap());
    let mut recs = vec![];
    for _ in 0..n {
        let head = take(&mut p, 8)?.try_into().unwrap();
        let mut s: [Vec<u8>; 3] = Default::default();
        for x in &mut s {
            let ln = u32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap()) as usize;
            *x = take(&mut p, ln)?.to_vec();
        }
        recs.push((head, s, take(&mut p, 32)?.try_into().unwrap()));
    }
    if p != d.len() {
        return fail(format!(
            "CoachtitleTableRecord.bin has {} bytes after its last title",
            d.len() - p
        ));
    }
    Ok(recs)
}

fn write(recs: &[Rec]) -> Vec<u8> {
    let mut out = (recs.len() as u32).to_le_bytes().to_vec();
    for (head, s, tail) in recs {
        out.extend(head);
        for x in s {
            out.extend((x.len() as u32).to_le_bytes());
            out.extend(x);
        }
        out.extend(tail);
    }
    out
}

/// Each title's and requirement's first choice that fits (printable ASCII within its room), else the next; none
/// fits = the Japanese stays. Read back. Returns the file, how many strings changed and the misfits.
fn pack(raw: &[u8], choose: impl Fn(&str) -> Vec<String>) -> R<(Vec<u8>, usize, Vec<String>)> {
    let old = read(raw)?;
    let mut recs = old.clone();
    let (mut done, mut misfit) = (0, vec![]);
    for (head, s, _) in &mut recs {
        let n = u32::from_le_bytes(head[..4].try_into().unwrap());
        for i in 0..2 {
            let id = format!("{}_{n:04}", PART[i]);
            let all = choose(&id);
            let fits = |t: &&String| {
                !t.is_empty() && t.len() <= ROOM[i] && t.bytes().all(|b| (b' '..0x7f).contains(&b))
            };
            if let Some(bad) = all.iter().find(|t| !fits(t)) {
                misfit.push(format!("{id} \"{bad}\""));
            }
            if let Some(t) = all.iter().find(fits) {
                s[i + 1] = t.as_bytes().to_vec();
                done += 1;
            }
        }
    }
    let new = write(&recs);
    if read(&new)? != recs
        || recs.len() != old.len()
        || recs
            .iter()
            .zip(&old)
            .any(|(a, b)| a.0 != b.0 || a.1[0] != b.1[0] || a.2 != b.2)
    {
        return fail("the built manager titles do not read back as intended - nothing changed");
    }
    Ok((new, done, misfit))
}

impl Ctx {
    /// The manager titles: Italian first for `it`, then English; a string with neither stays Japanese.
    pub(super) fn build_titles(&self, lang: Lang) -> R<Built> {
        let raw = self.original_file(TITLES)?;
        if sha(&raw) != TITLES_SHA {
            return fail(format!(
                "{TITLES} is not the original 2.00.00 file - the English is made for that only"
            ));
        }
        let load = |folder, col| -> R<std::collections::HashMap<String, String>> {
            Ok(self
                .sheet("coach_titles.tsv", folder)?
                .iter()
                .map(|r| (cell(r, "key").to_string(), cell(r, col).to_string()))
                .collect())
        };
        let en = load("english", "english")?;
        let it = if lang == Lang::It {
            load("italian", "italiano")?
        } else {
            Default::default()
        };
        let (new, done, misfit) = pack(&raw, |k| {
            [it.get(k), en.get(k)]
                .into_iter()
                .flatten()
                .cloned()
                .collect()
        })?;
        let mut note = format!("{done} manager title texts in {}", lang.name());
        if let Some(m) = misfit.first() {
            note += &format!(
                "; {} do not fit and gave way to the next choice ({m})",
                misfit.len()
            );
        }
        Ok((vec![(TITLES.to_string(), new)], note))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Edition, spec};
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Two titles, the second's strings in cp932.
    fn table() -> Vec<u8> {
        let (jp_t, _, _) = encoding_rs::SHIFT_JIS.encode("至宝");
        let (jp_r, _, _) = encoding_rs::SHIFT_JIS.encode("所属選手1人が追加能力を獲得");
        write(&[
            (
                [0, 0, 0, 0, 1, 0, 0, 0],
                [b"None".to_vec(), b"x".to_vec(), b"y".to_vec()],
                [0; 32],
            ),
            (
                [1, 0, 0, 0, 1, 0, 0, 0],
                [b"Shihou".to_vec(), jp_t.into(), jp_r.into()],
                [7; 32],
            ),
        ])
    }

    #[test]
    fn round_trip_and_swap() {
        let raw = table();
        assert_eq!(write(&read(&raw).unwrap()), raw);
        let (new, done, misfit) = pack(&raw, |k| match k {
            "title_0001" => vec!["Treasure".into()],
            "requirement_0001" => vec!["1 squad player gains an Extra Ability".into()],
            _ => vec![],
        })
        .unwrap();
        assert_eq!((done, misfit.len()), (2, 0));
        let r = read(&new).unwrap();
        assert_eq!(
            r[0],
            read(&raw).unwrap()[0],
            "no choice: the original stays"
        );
        assert_eq!(r[1].1[1], b"Treasure");
        assert_eq!(r[1].1[2], b"1 squad player gains an Extra Ability");
        assert_eq!((r[1].0, r[1].2), ([1, 0, 0, 0, 1, 0, 0, 0], [7; 32]));
        assert!(read(&raw[..raw.len() - 1]).is_err());
    }

    #[test]
    fn too_long_or_not_ascii_gives_way() {
        let raw = table();
        let (new, done, misfit) = pack(&raw, |k| match k {
            "title_0001" => vec!["A".repeat(25), "Treasure".into()],
            "requirement_0001" => vec!["Abilita\u{e0}".into(), "B".repeat(63)],
            _ => vec![],
        })
        .unwrap();
        assert_eq!((done, misfit.len()), (1, 2));
        let r = read(&new).unwrap();
        assert_eq!(r[1].1[1], b"Treasure");
        assert_eq!(
            r[1].1[2],
            read(&raw).unwrap()[1].1[2],
            "nothing fits: Japanese stays"
        );
        assert!(pack(&raw, |_| vec!["A".repeat(24)]).unwrap().2.is_empty());
    }

    #[test]
    fn on_then_off_restores_the_bytes() {
        let t = tempfile::tempdir().unwrap();
        let (game, backup) = (t.path().join("game"), t.path().join("backup"));
        let raw = table();
        fs::create_dir_all(game.join("prog_data/enumdata")).unwrap();
        fs::write(game.join(TITLES), &raw).unwrap();
        let mut c = Ctx {
            ed: Edition::E1718,
            spec: spec(Edition::E1718),
            game: game.clone(),
            backup: backup.clone(),
            sheets: t.path().join("sheets"),
            cpu: vec![],
        };
        // the synthetic table stands in for the original
        let h: &'static str = Box::leak(sha(&raw).into_boxed_str());
        c.spec.originals = vec![(TITLES, h)];
        let (new, _, _) = pack(&raw, |_| vec!["T".into()]).unwrap();
        c.turn_on(&[(TITLES.to_string(), new.clone())], Lang::En)
            .unwrap();
        assert_eq!(fs::read(game.join(TITLES)).unwrap(), new);
        assert_eq!(fs::read(backup.join(TITLES)).unwrap(), raw);
        assert_eq!(c.turn_off().unwrap(), 1);
        assert_eq!(fs::read(game.join(TITLES)).unwrap(), raw);
        assert!(!Path::new(&backup).exists());
    }

    /// `FORZA_TITLES=<copy of the game's file> FORZA_SHEETS=<editions/2017-18> cargo test -- --ignored`: English
    /// and Italian beside the copy (`<file>.en`, `<file>.it`).
    #[test]
    #[ignore]
    fn the_real_table() {
        let f = std::path::PathBuf::from(std::env::var("FORZA_TITLES").unwrap());
        let game = f.parent().unwrap().join("game");
        fs::create_dir_all(game.join("prog_data/enumdata")).unwrap();
        fs::copy(&f, game.join(TITLES)).unwrap();
        let c = Ctx {
            ed: Edition::E1718,
            spec: spec(Edition::E1718),
            game,
            backup: f.parent().unwrap().join("none"),
            sheets: std::env::var("FORZA_SHEETS").unwrap().into(),
            cpu: vec![],
        };
        for lang in [Lang::En, Lang::It] {
            let (files, note) = c.build_titles(lang).unwrap();
            println!("{note}");
            assert!(!note.contains("do not fit"), "{note}");
            fs::write(f.with_extension(lang.code()), &files[0].1).unwrap();
        }
    }
}
