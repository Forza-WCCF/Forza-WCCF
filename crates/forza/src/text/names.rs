use super::*;

// ---------------------------------------------------------------- player and team names
pub(crate) fn fold(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| match c {
            'Ø' => "O".to_string(),
            'ø' => "o".into(),
            'Æ' => "AE".into(),
            'æ' => "ae".into(),
            'ß' => "SS".into(),
            'Ł' => "L".into(),
            'ł' => "l".into(),
            'Đ' => "D".into(),
            'đ' => "d".into(),
            'Œ' => "OE".into(),
            'œ' => "oe".into(),
            'ı' => "i".into(),
            'Þ' => "TH".into(),
            'ð' => "d".into(),
            c => c.to_string(),
        })
        .collect();
    s.nfkd()
        .filter(|&c| canonical_combining_class(c) == 0)
        .map(|c| if (' '..'\u{7f}').contains(&c) { c } else { '?' })
        .collect()
}

/// A name field holds [`FIELD`] bytes and a closing end byte, so a longer folded name is cut; and `fold` writes
/// `?` for a letter the game's font has no glyph for.
#[derive(Default)]
pub(super) struct NameFaults {
    pub(super) cut: usize,
    pub(super) unknown: usize,
}

impl NameFaults {
    pub(super) fn add(&mut self, name: &str) {
        if name.len() >= FIELD {
            self.cut += 1;
        }
        if name.contains('?') {
            self.unknown += 1;
        }
    }

    /// The player data's status note: the counts, then the names that did not go in as they are (`skipped`: the team
    /// files that are not original ones). The keys hold every piece; one that does not apply is empty.
    pub(super) fn note(&self, players: usize, teams: usize, skipped: &[String]) -> Line {
        let no = Line::raw("");
        let mut note = Line::key("text.note_players")
            .arg("players", players)
            .arg("teams", teams)
            .part(
                "skipped",
                match skipped.first() {
                    Some(first) => Line::key("text.note_team_files")
                        .arg("count", skipped.len())
                        .arg("first", first),
                    None => no.clone(),
                },
            );
        for (count, key, slot) in [
            (self.cut, "text.note_cut", "cut"),
            (self.unknown, "text.note_unknown", "unknown"),
        ] {
            note = note.part(
                slot,
                if count > 0 {
                    Line::key(key).arg("count", count)
                } else {
                    no.clone()
                },
            );
        }
        note
    }
}

/// value + end byte, then the field's own fill (whatever followed the original end byte).
pub(super) fn put_field(field: &[u8], value: &[u8], size: usize) -> Vec<u8> {
    let fill = match field.iter().position(|&b| b == 0) {
        Some(e) if e + 1 < size => field[e + 1],
        _ => 0,
    };
    let mut out: Vec<u8> = value[..value.len().min(size - 1)].to_vec();
    out.push(0);
    out.resize(size, fill);
    out
}

/// A field's text up to its first end byte.
pub(super) fn first(f: &[u8]) -> &[u8] {
    f.split(|&b| b == 0).next().unwrap_or(&[])
}

fn utf16(f: &[u8]) -> String {
    let u: Vec<u16> = f
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&u)
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .into()
}

/// Each player record's short name in folded Latin (the kana initial in front, as the game's card shows it), its
/// card name the full Latin name and its Japanese skill name the English one; empty records stay as they are.
pub(super) fn rename_players(pd: &mut [u8], s: &Spec) -> NameFaults {
    let set = |pd: &mut [u8], at: usize, v: &[u8]| {
        let n = put_field(&pd[at..at + FIELD], v, FIELD);
        pd[at..at + FIELD].copy_from_slice(&n);
    };
    let mut names = NameFaults::default();
    for i in 0..pd.len() / s.rec {
        let r = i * s.rec;
        let kana = cp932(first(&pd[r + s.short_kana..r + s.short_kana + FIELD]));
        let mut latin = utf16(&pd[r + s.short_latin..r + s.short_latin + FIELD]);
        if let (true, Some(d)) = (latin.is_empty(), s.display) {
            latin = String::from_utf8_lossy(first(&pd[r + d..r + d + FIELD])).into_owned();
        }
        if latin.is_empty() {
            continue;
        }
        let mut name = fold(&latin);
        let mut kc = kana.chars();
        if let (Some(a @ 'Ａ'..='Ｚ'), Some('．')) = (kc.next(), kc.next()) {
            let ini = format!("{}.", char::from_u32(a as u32 - 0xFEE0).unwrap());
            if !name.starts_with(&ini) {
                name = ini + &name;
            }
        }
        names.add(&name);
        set(pd, r + s.short_kana, name.as_bytes());
        if let Some((full_latin, card_name)) = s.full {
            let full = fold(&utf16(&pd[r + full_latin..r + full_latin + FIELD]));
            if !full.is_empty() {
                names.add(&full);
                set(pd, r + card_name, full.as_bytes());
            }
        }
        let sk = first(&pd[r + s.skill_en..r + s.skill_en + FIELD]).to_vec();
        if !sk.is_empty() && sk.iter().all(|c| (32..127).contains(c)) && !sk.contains(&b'%') {
            set(pd, r + s.skill_jp, &sk);
        }
    }
    names
}

impl Ctx {
    pub(super) fn build_names(&self, lang: Lang) -> R<Built> {
        let s = &self.spec;
        let mut pd = self.original_file(PLAYERS)?;
        if Some(sha(&pd)) != self.original_hash(PLAYERS) || pd.len() % s.rec != 0 {
            return fail(
                Line::key("text.fail_not_original")
                    .arg("file", "prog_data\\player_data\\player_data.bin")
                    .arg(
                        "edition",
                        match self.ed {
                            Edition::E1011 => "Rev D",
                            Edition::E1718 => "2.00.00",
                        },
                    ),
            );
        }
        let names = rename_players(&mut pd, s);
        let n_players = pd.len() / s.rec;
        let mut files = vec![(PLAYERS.to_string(), pd)];
        let mut skipped = vec![];
        for (rel, h, en, it) in &self.cpu {
            let file = rel.rsplit('/').next().unwrap_or(rel);
            let en = if lang == Lang::It && !it.is_empty() {
                it
            } else {
                en
            };
            let mut d = self.original_file(rel)?;
            if sha(&d) != *h {
                skipped.push(file.to_string());
                continue;
            }
            if !team_name_fits(en) {
                return fail(Line::key("text.fail_team_name").arg("file", file));
            }
            let n = put_field(&d[NAME_AT..NAME_END], en.as_bytes(), NAME_END - NAME_AT);
            d[NAME_AT..NAME_END].copy_from_slice(&n);
            files.push((rel.clone(), d));
        }
        let note = names.note(n_players, files.len() - 1, &skipped);
        Ok((files, vec![note]))
    }
}
