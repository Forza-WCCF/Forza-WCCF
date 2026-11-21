use super::*;

pub(super) const REC_1718: usize = 0x620;
const RECORDS_1718: usize = 8434;
const CARD_TABLE_VA_1718: u32 = 0x015B_5B48;
const CARD_TABLE_N_1718: usize = 0x2CC1;
/// VerName.txt line 13 (0-based): "not players" (referees, coaches, GUESTPLAY stand-ins, the navigator).
const PLACEHOLDER_EDITION: u8 = 13;
/// The publisher's own joke cards: 3509's picture is a hand-scrawled "dummy", 9951 a staff Maradona parody.
const NOT_PLAYERS: [usize; 2] = [3509, 9951];

/// The 2017-18 catalogue's columns: 0-30 as 2010-11 writes them (the overlay reads them by position), then this
/// edition's own raw fields.
pub(super) const COLS_1718: &[&str] = &[
    "card_no",
    "record_index",
    "set_no",
    "edition",
    "edition_label",
    "season",
    "rarity",
    "rarity_name",
    "rarity_frame",
    "is_placeholder",
    "name_full_latin",
    "name_short_latin",
    "name_display_ascii",
    "name_full_kana",
    "name_short_kana",
    "position_name",
    "club_id",
    "club_name",
    "nationality_id",
    "nationality_name",
    "shirt_numbers",
    "birth_date",
    "height_cm",
    "weight_kg",
    "stat_offence",
    "stat_defence",
    "stat_technique",
    "stat_power",
    "stat_speed",
    "stat_stamina",
    "stats_total",
    "skill_en",
    "skill_jp",
    "model_key",
    "image_archive_path",
    "image_xaf_offset",
    "image_xaf_stored_size",
    "image_size",
    "image_packed",
    "card_code",
    "club_name_jp",
    "nationality_name_jp",
    "u16_28e_298",
    "bytes_3cd_3d6",
    "byte_3de",
    "hidden_4df_523",
    "tail_5a4_61f",
];

/// Cards whose back was read by eye: (number, short name, line, club, nationality, six stats, birth date, height,
/// weight). 1007 and 4101 were read on 2026-10-07, the rest on 2026-10-08.
type Known = (
    usize,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    [u8; 6],
    &'static str,
    u8,
    u8,
);
const KNOWN: &[Known] = &[
    (
        7049,
        "MESSI",
        "FW",
        "FC Barcelona",
        "Argentina",
        [20, 8, 20, 16, 19, 15],
        "1987-06-24",
        170,
        72,
    ),
    (
        7064,
        "RONALDO",
        "FW",
        "Real Madrid",
        "Portugal",
        [20, 5, 19, 19, 19, 15],
        "1985-02-05",
        185,
        80,
    ),
    (
        6840,
        "M.SALAH",
        "FW",
        "Liverpool",
        "Egypt",
        [16, 8, 17, 14, 19, 15],
        "1992-06-15",
        175,
        71,
    ),
    (
        6904,
        "LEWANDOWSKI",
        "FW",
        "Bayern Munchen",
        "Poland",
        [19, 6, 17, 18, 16, 16],
        "1988-08-21",
        185,
        79,
    ),
    (
        7061,
        "MODRIĆ",
        "MF",
        "Real Madrid",
        "Croatia",
        [16, 12, 19, 13, 17, 17],
        "1985-09-09",
        174,
        65,
    ),
    (
        6883,
        "KAGAWA",
        "MF",
        "Borussia Dortmund",
        "Japan",
        [16, 10, 17, 13, 16, 16],
        "1989-03-17",
        175,
        68,
    ),
    (
        7054,
        "SERGIO RAMOS",
        "DF",
        "Real Madrid",
        "Spain",
        [12, 19, 14, 18, 15, 17],
        "1986-03-30",
        183,
        75,
    ),
    (
        7127,
        "NEUER",
        "GK",
        "Bayern Munchen",
        "Germany",
        [10, 20, 14, 18, 14, 13],
        "1986-03-27",
        193,
        92,
    ),
    (
        6954,
        "BUFFON",
        "GK",
        "Juventus",
        "Italy",
        [8, 20, 13, 18, 13, 13],
        "1978-01-28",
        191,
        91,
    ),
    (
        5980,
        "BUFFON",
        "GK",
        "Juventus",
        "Italy",
        [8, 20, 13, 18, 13, 13],
        "1978-01-28",
        192,
        94,
    ),
    (
        6826,
        "MIGNOLET",
        "GK",
        "Liverpool",
        "Belgium",
        [7, 18, 12, 18, 12, 11],
        "1988-03-06",
        193,
        87,
    ),
    (
        1007,
        "BA",
        "FW",
        "Chelsea",
        "Senegal",
        [19, 5, 15, 18, 17, 14],
        "1985-05-25",
        189,
        85,
    ),
    (
        4101,
        "SZCZESNY",
        "GK",
        "Arsenal",
        "Poland",
        [7, 18, 12, 18, 12, 11],
        "1990-04-18",
        195,
        93,
    ),
];

/// A 2017-18 card: what its row and the checks use.
struct Card1718 {
    cells: Vec<String>,
    card_no: usize,
    placeholder: bool,
    season: String,
    position: u8,
    rarity: u8,
    rarity_name: &'static str,
    stats: [u8; 6],
    stats_total: u8,
    birth: String,
    short: String,
    pos_name: &'static str,
    club: String,
    nat: String,
    height: u8,
    weight: u8,
    image: Option<(u64, u32, u32)>,
    image_path: String,
}

/// The 286 Japanese club (0-189) and nationality (0-95) names the exe points to.
fn exe_names(d: &[u8]) -> io::Result<Vec<String>> {
    let n = CLUBS_1718.len() + NATIONS_1718.len();
    let off = va_to_offset(d, NAME_TABLES_1718[0])?;
    if d.len() < off + 4 * n {
        return Err(bad("name table runs past the file"));
    }
    (0..n)
        .map(|i| {
            let o = va_to_offset(d, u32_at(d, off + 4 * i))?;
            let end = d[o..]
                .iter()
                .position(|&c| c == 0)
                .ok_or_else(|| bad("name without an end"))?;
            Ok(encoding_rs::SHIFT_JIS
                .decode_without_bom_handling(&d[o..o + end])
                .0
                .into_owned())
        })
        .collect()
}

/// VerName.txt -> edition number -> label; each line is "<name> t|f" (the original flag, unused here).
pub(super) fn editions_1718(path: &Path) -> io::Result<HashMap<u8, String>> {
    let raw = fs::read(path)?;
    let (text, _, err) = encoding_rs::SHIFT_JIS.decode(&raw);
    if err {
        return Err(bad("VerName.txt is not Shift-JIS"));
    }
    Ok(text
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let t = l.trim();
            let t = match t.strip_suffix(['t', 'f']) {
                Some(h) if h.ends_with(char::is_whitespace) => h.trim_end(),
                _ => t,
            };
            let label = t
                .replace("ロケテスト", " location test")
                .replace("選手以外", "not players")
                .replace("未使用", "unused")
                .replace("テスト", "test");
            (i as u8, label)
        })
        .collect())
}

/// "AOU2002" and "Camp NAOMI" have no years in the name.
pub(super) fn season_of(edition: u8, label: &str) -> String {
    match edition {
        1 => return "2001-02".into(),
        9 => return "2004-05/2005-06".into(),
        _ => {}
    }
    // "dddd-dddd" anywhere in the label -> "dddd-" + the last two digits
    label
        .as_bytes()
        .windows(9)
        .find(|w| {
            w[4] == b'-'
                && w.iter()
                    .enumerate()
                    .all(|(i, c)| i == 4 || c.is_ascii_digit())
        })
        .map(|w| {
            format!(
                "{}-{}",
                String::from_utf8_lossy(&w[..4]),
                String::from_utf8_lossy(&w[7..9])
            )
        })
        .unwrap_or_default()
}

/// One 0x620-byte record's raw fields (the 2010-11 record moved to +0x288, the club widened to a u16 at +0x28C, the
/// card code added at +0x2A0: proved against the 3,924 cards both games share).
pub(super) struct Rec1718 {
    pub(super) card_no: usize,
    set_no: u16,
    pub(super) club_id: u16,
    pub(super) u16s: Vec<u16>,
    pub(super) edition: u8,
    pub(super) rarity: u8,
    card_code: String,
    full_latin: String,
    full_kana: String,
    short_kana: String,
    pub(super) short_latin: String,
    shirts: Vec<u8>,
    pub(super) position: u8,
    pub(super) birth_raw: String,
    height: u8,
    weight: u8,
    nat: u8,
    pub(super) stats: [u8; 6],
    total: u8,
    display: String,
    skill_en: String,
    skill_jp: String,
    model_key: String,
    pub(super) image_name: String,
}

pub(super) fn parse_1718(r: &[u8]) -> Rec1718 {
    let u: Vec<u16> = (0..11).map(|i| u16_at(r, 0x288 + 2 * i)).collect();
    Rec1718 {
        card_no: usize::from(u[0]),
        set_no: u[1],
        club_id: u[2],
        u16s: u[3..].to_vec(),
        edition: r[0x29E],
        rarity: r[0x29F],
        card_code: cp932(r, 0x2A0, 0x10),
        full_latin: utf16(r, 0x2B0, 0x40),
        full_kana: cp932(r, 0x2F0, 0x40),
        short_kana: cp932(r, 0x330, 0x40),
        short_latin: utf16(r, 0x370, 0x40),
        shirts: r[0x3B0..0x3B9].to_vec(),
        position: r[0x3B9],
        birth_raw: ascii(r, 0x3BA, 0x10),
        height: r[0x3CA],
        weight: r[0x3CB],
        nat: r[0x3CC],
        stats: r[0x3D7..0x3DD].try_into().unwrap(),
        total: r[0x3DD],
        display: ascii(r, 0x3DF, 0x40),
        skill_en: ascii(r, 0x45F, 0x40),
        skill_jp: cp932(r, 0x49F, 0x40),
        model_key: ascii(r, 0x524, 0x40),
        image_name: ascii(r, 0x564, 0x40),
    }
}

pub(super) fn build_rows_1718(files: &GameFiles, checks: &mut Checks) -> io::Result<Built> {
    let mut check =
        |name: &str, ok: bool, detail: String| checks.push((name.to_string(), ok, detail));
    let exe = read_exe(&files.exe)?;
    let table = card_table(&exe, CARD_TABLE_VA_1718, CARD_TABLE_N_1718)?;
    let names = exe_names(&exe)?;
    let data = fs::read(&files.players)?;
    check(
        "player_data.bin is whole 0x620-byte records",
        data.len() % REC_1718 == 0,
        format!("{} bytes", data.len()),
    );
    let recs: Vec<&[u8]> = data
        .as_chunks::<REC_1718>()
        .0
        .iter()
        .map(|c| c.as_slice())
        .collect();
    check(
        &format!("record count {RECORDS_1718}"),
        recs.len() == RECORDS_1718,
        format!("{} records", recs.len()),
    );
    let mut vals: Vec<usize> = table.values().copied().collect();
    vals.sort_unstable();
    check(
        "the exe's table maps one card number to each record",
        vals.iter().copied().eq(0..recs.len()),
        format!("{} card numbers", table.len()),
    );
    let jp = CLUBS_1718.iter().chain(NATIONS_1718).map(|t| t.0);
    let differ: Vec<usize> = jp
        .zip(&names)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .take(10)
        .collect();
    check(
        "the exe's club and nationality names are the ones translated here",
        differ.is_empty(),
        format!("differ at {differ:?}"),
    );
    let eds = editions_1718(&files.vername)?;
    let ents = xaf_toc(&files.xaf)?;
    let mut images = HashMap::new();
    let mut faces = HashSet::new();
    for (i, e) in ents.iter().enumerate().filter(|(_, e)| e.is_file) {
        if e.path.starts_with("data/player_card/") {
            images.insert(e.name.to_lowercase(), i);
        }
        if e.path.starts_with("data/models/face/") {
            faces.insert(e.name.to_lowercase());
        }
    }
    let (mut cards, mut mism, mut exact_case, mut face_ok, mut beyond) = (Vec::new(), 0, 0, 0, 0);
    let mut prefix_bad = Vec::new();
    for (&card_no, &idx) in &table {
        let Some(r) = recs.get(idx) else {
            beyond += 1;
            continue;
        };
        let f = parse_1718(r);
        mism += usize::from(f.card_no != card_no);
        let placeholder = f.edition == PLACEHOLDER_EDITION || NOT_PLAYERS.contains(&card_no);
        let (rname, rframe) = rarity(f.rarity);
        let elabel = eds.get(&f.edition).map_or("", String::as_str);
        let season = if placeholder {
            String::new()
        } else {
            season_of(f.edition, elabel)
        };
        // 7 placeholders name no picture: the game's error card
        let image = if f.image_name.is_empty() {
            "ERROR_CARD"
        } else {
            &f.image_name
        };
        let img = images
            .get(&format!("{image}.dds").to_lowercase())
            .map(|&i| &ents[i]);
        exact_case += usize::from(img.is_some_and(|e| e.name == format!("{image}.dds")));
        face_ok += usize::from(faces.contains(&format!("{}.svo", f.model_key).to_lowercase()));
        if f.edition >= 19
            && season.len() >= 7
            && !image.starts_with(&format!("{}{}", &season[2..4], &season[5..7]))
        {
            prefix_bad.push(card_no);
        }
        let club = CLUBS_1718
            .get(usize::from(f.club_id))
            .copied()
            .unwrap_or(("", "", ""));
        let nat = NATIONS_1718
            .get(usize::from(f.nat))
            .copied()
            .unwrap_or(("", "", ""));
        let (club_name, nat_name) = if placeholder {
            ("", "")
        } else {
            (club.1, nat.1)
        };
        let pos_name = POSITIONS
            .get(usize::from(f.position))
            .copied()
            .unwrap_or("");
        let birth = birth_iso(&f.birth_raw);
        let image_path = format!("data/player_card/{image}.dds");
        let b = |v: bool| if v { "1" } else { "0" }.to_string();
        let opt = |v: Option<String>| v.unwrap_or_default();
        let u16s: Vec<String> = f.u16s.iter().map(u16::to_string).collect();
        let mut cells = vec![
            card_no.to_string(),
            idx.to_string(),
            f.set_no.to_string(),
            f.edition.to_string(),
            elabel.to_string(),
            season.clone(),
            f.rarity.to_string(),
            rname.to_string(),
            rframe.to_string(),
            b(placeholder),
            f.full_latin,
            f.short_latin.clone(),
            f.display,
            f.full_kana,
            f.short_kana,
            pos_name.to_string(),
            f.club_id.to_string(),
            club_name.to_string(),
            f.nat.to_string(),
            nat_name.to_string(),
            list(&f.shirts),
            birth.clone(),
            f.height.to_string(),
            f.weight.to_string(),
        ];
        cells.extend(f.stats.iter().map(u8::to_string));
        cells.extend([
            f.total.to_string(),
            f.skill_en,
            f.skill_jp,
            f.model_key,
            image_path.clone(),
            opt(img.map(|e| e.offset.to_string())),
            opt(img.map(|e| e.stored.to_string())),
            opt(img.map(|e| e.size.to_string())),
            opt(img.map(|e| b(e.stored != e.size))),
            f.card_code,
            club.0.to_string(),
            nat.0.to_string(),
            u16s.join(" "),
            list(&r[0x3CD..0x3D7]),
            r[0x3DE].to_string(),
            list(&r[0x4DF..0x524]),
            r[0x5A4..0x620].iter().map(|b| format!("{b:02x}")).collect(),
        ]);
        cards.push(Card1718 {
            cells,
            card_no,
            placeholder,
            season,
            position: f.position,
            rarity: f.rarity,
            rarity_name: rname,
            stats: f.stats,
            stats_total: f.total,
            birth,
            short: f.short_latin,
            pos_name,
            club: club_name.to_string(),
            nat: nat_name.to_string(),
            height: f.height,
            weight: f.weight,
            image: img.map(|e| (e.offset, e.stored, e.size)),
            image_path,
        });
    }
    let n = cards.len();
    let real: Vec<&Card1718> = cards.iter().filter(|c| !c.placeholder).collect();
    check(
        "every card number in the exe's table has its record",
        beyond == 0,
        format!("{beyond} point past the file's end"),
    );
    check(
        "the number at +0x288 of each record is its card number",
        mism == 0,
        format!("{mism} differ"),
    );
    check(
        "every card's picture is in the archive",
        cards.iter().all(|c| c.image.is_some()),
        String::new(),
    );
    check(
        "picture names match the archive's case",
        exact_case == n,
        format!("{exact_case}/{n}"),
    );
    check(
        "every 2009-10+ picture name starts with its season (e.g. 1718_)",
        prefix_bad.is_empty(),
        format!("{:?}", &prefix_bad[..prefix_bad.len().min(10)]),
    );
    // card 443 (GARGO) stores 77 while its six sum to 78 - the same in 2005-06 and 2010-11, so it is the original data
    let off_sum: Vec<usize> = real
        .iter()
        .filter(|c| c.stats.iter().map(|&s| u32::from(s)).sum::<u32>() != u32::from(c.stats_total))
        .map(|c| c.card_no)
        .collect();
    check(
        "total = the six stats added up (except the original card 443)",
        off_sum.is_empty() || off_sum == [443],
        format!("{off_sum:?}"),
    );
    check(
        "every card's face model is in the archive",
        face_ok == n,
        format!("{face_ok}/{n}"),
    );
    check(
        "every position is GK, DF, MF or FW",
        cards.iter().all(|c| c.position < 4),
        String::new(),
    );
    check(
        "every real card's rarity is 1..5",
        real.iter().all(|c| (1..=5).contains(&c.rarity)),
        String::new(),
    );
    check(
        "every real card's edition has a season",
        real.iter().all(|c| !c.season.is_empty()),
        String::new(),
    );
    check(
        "every real card's stat is 1..20, total 60..100",
        real.iter().all(|c| {
            c.stats.iter().all(|s| (1..=20).contains(s)) && (60..=100).contains(&c.stats_total)
        }),
        String::new(),
    );
    let nobirth = real.iter().filter(|c| c.birth.is_empty()).count();
    check(
        "every real card's birth date reads",
        nobirth == 0,
        format!("{nobirth} do not"),
    );
    check(
        "card numbers are unique",
        cards
            .iter()
            .map(|c| c.card_no)
            .collect::<HashSet<_>>()
            .len()
            == n,
        String::new(),
    );
    let noname: Vec<usize> = real
        .iter()
        .filter(|c| c.club.is_empty() || c.nat.is_empty())
        .map(|c| c.card_no)
        .collect();
    check(
        "every real card's club and nationality have a name",
        noname.is_empty(),
        format!("{:?}", &noname[..noname.len().min(10)]),
    );
    let by_no: HashMap<usize, &Card1718> = cards.iter().map(|c| (c.card_no, c)).collect();
    for &(no, short, pos, club, nat, stats, birth, h, w) in KNOWN {
        let c = by_no.get(&no);
        let ok = c.is_some_and(|c| {
            (
                c.short.as_str(),
                c.pos_name,
                c.club.as_str(),
                c.nat.as_str(),
            ) == (short, pos, club, nat)
                && c.stats == stats
                && (c.birth.as_str(), c.height, c.weight) == (birth, h, w)
        });
        check(&format!("card {no} is {short}"), ok, String::new());
    }
    check(
        "cards 1 (referee/coach stand-in), 10570 (GUESTPLAY1) and 3509 (dummy) are placeholders",
        [1, 10570, 3509]
            .iter()
            .all(|n| by_no.get(n).is_some_and(|c| c.placeholder)),
        String::new(),
    );
    // its back prints "SECRET" and no stats: the record holds 16 16 16 16 16 20
    check(
        "card 7143 is MARADONA, ALL TIME LEGEND, stored stats 16x5 + 20",
        by_no.get(&7143).is_some_and(|c| {
            c.short == "MARADONA"
                && c.rarity_name == "ALL TIME LEGEND"
                && c.stats == [16, 16, 16, 16, 16, 20]
        }),
        String::new(),
    );
    // newest season first (stable): the overlay keeps only the first 4,096 non-placeholder lines, so this order
    // keeps 2012-13 .. 2017-18
    cards.sort_by(|a, b| b.season.cmp(&a.season));
    let toc = ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_file && e.path.starts_with("data/player_card/"))
        .map(|(i, e)| (e.path.clone(), i))
        .collect();
    let rows = cards
        .into_iter()
        .map(|c| Row {
            cells: c.cells,
            card_no: c.card_no,
            placeholder: c.placeholder,
            image: c.image,
            image_path: c.image_path,
            ..Row::default()
        })
        .collect();
    Ok((rows, toc, ents))
}
