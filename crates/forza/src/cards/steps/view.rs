//! data\club_view.txt, the club card as the game reads it, for the CLUB CARD panel (port of
//! `legacy/2010-11/code/cards/view.py`; the lines are described in [`crate::club_view`], which reads them). ASCII for
//! the overlay's font: kana names in romaji, accents dropped as the English text drops them. Never fails for a bad card:
//! it says what is wrong, and the other cards and the session line come with every state (never locked out).

use super::decode::{self as d, Decoded};
use super::wallet;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, SystemTime};
use unicode_normalization::UnicodeNormalization as _;

const KANA: &str = "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲンガギグゲゴザジズゼゾダヂヅデドバビブベボパピプペポヴ";
const ROMAJI: &str = "a i u e o ka ki ku ke ko sa shi su se so ta chi tsu te to na ni nu ne no ha hi fu he ho ma mi mu me mo ya yu yo ra ri ru re ro wa wo n ga gi gu ge go za ji zu ze zo da ji zu de do ba bi bu be bo pa pi pu pe po vu";

fn small(c: char) -> Option<&'static str> {
    Some(match c {
        'ャ' => "ya",
        'ュ' => "yu",
        'ョ' => "yo",
        'ァ' => "a",
        'ィ' => "i",
        'ゥ' => "u",
        'ェ' => "e",
        'ォ' => "o",
        _ => return None,
    })
}

fn ends_vowel(s: &str) -> bool {
    s.ends_with(['a', 'i', 'u', 'e', 'o'])
}

/// Katakana (hiragana made katakana first) in Hepburn romaji: small ya/yu/yo and small vowels join the syllable before
/// them, a small tsu doubles the next consonant, the long mark repeats the vowel before it.
fn romaji(s: &str) -> String {
    let kana: HashMap<char, &str> = KANA.chars().zip(ROMAJI.split(' ')).collect();
    let (mut out, mut double): (Vec<String>, bool) = (Vec::new(), false);
    for mut ch in s.chars() {
        if ('ぁ'..='ゖ').contains(&ch) {
            ch = char::from_u32(ch as u32 + 0x60).unwrap();
        }
        if ch == 'ッ' {
            double = true;
            continue;
        }
        if let Some(sm) = small(ch)
            && out
                .last()
                .is_some_and(|p| ends_vowel(p) && p.chars().all(char::is_alphabetic))
        {
            let prev = out.pop().unwrap();
            let stem = &prev[..prev.len() - 1];
            if sm.len() == 2 {
                let y = if ["sh", "ch", "j"].contains(&stem) {
                    ""
                } else {
                    "y"
                };
                out.push(format!("{stem}{y}{}", &sm[1..]));
            } else {
                out.push(format!("{stem}{sm}"));
            }
            continue;
        }
        if ch == 'ー' {
            let v = out
                .last()
                .filter(|p| ends_vowel(p))
                .map_or("-".into(), |p| p[p.len() - 1..].to_owned());
            out.push(v);
            continue;
        }
        let Some(&r) = kana.get(&ch) else {
            out.push(ch.into());
            double = false;
            continue;
        };
        let r = if double && !r.starts_with(['a', 'i', 'u', 'e', 'o', 'n']) {
            format!("{}{r}", &r[..1])
        } else {
            r.to_owned()
        };
        double = false;
        out.push(r);
    }
    out.concat()
}

/// A name as the panel can draw it: full-width letters made plain, kana in romaji (capitals), accents dropped as the
/// English text drops them, anything else "?" (view.ascii_name).
pub fn ascii_name(s: &str) -> String {
    let mut s: String = s.nfkc().collect();
    if s.chars().any(|c| ('ぁ'..='ヿ').contains(&c)) {
        s = romaji(&s).to_uppercase();
    }
    crate::text::fold(&s).replace('|', "?").trim().to_owned()
}

/// A stored money value ($100 units) in dollars, with thousands commas.
fn money(v: u32) -> String {
    let digits = (u64::from(v) * 100).to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("${out}")
}

pub fn ordinal(n: u32) -> String {
    let suffix = match n % 10 {
        _ if (10..=20).contains(&(n % 100)) => "th",
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// {card number: (name, position)} from catalogue.tsv: the short Latin name, made ASCII (view.load_names).
pub fn load_names(path: &Path) -> HashMap<u32, (String, String)> {
    let mut out = HashMap::new();
    let Ok(mut r) = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .flexible(true)
        .from_path(path)
    else {
        return out;
    };
    let Ok(head) = r.headers().cloned() else {
        return out;
    };
    let ix = |n: &str| head.iter().position(|h| h == n);
    let (no_ix, short, ascii, pos) = (
        ix("card_no"),
        ix("name_short_latin"),
        ix("name_display_ascii"),
        ix("position_name"),
    );
    for row in r.records().flatten() {
        let get = |i: Option<usize>| i.and_then(|i| row.get(i)).unwrap_or("");
        let Ok(no) = get(no_ix).trim().parse::<u32>() else {
            continue;
        };
        let name = Some(get(short))
            .filter(|s| !s.is_empty())
            .unwrap_or(get(ascii));
        out.insert(no, (ascii_name(name), get(pos).trim().to_owned()));
    }
    out
}

/// A file time as `YYYY-MM-DD HH:MM` on this PC's clock.
fn stamp(t: SystemTime) -> String {
    let utc = time::OffsetDateTime::from(t);
    let t = time::UtcOffset::local_offset_at(utc).map_or(utc, |o| utc.to_offset(o));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

fn mtime(p: &Path) -> io::Result<SystemTime> {
    fs::metadata(p)?.modified()
}

/// The view's lines for this card file and the other cards in `cards` (view.build).
pub fn build(
    card: &Path,
    catalogue: &Path,
    backups: &Path,
    running: &Path,
    cards: &Path,
) -> io::Result<Vec<String>> {
    let mut found: Vec<String> = if backups.is_dir() {
        fs::read_dir(backups)?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|f| f.ends_with(".bak"))
            .collect()
    } else {
        Vec::new()
    };
    found.sort_unstable_by(|a, b| b.cmp(a));
    let back = found
        .iter()
        .take(6)
        .map(|f| mtime(&backups.join(f)).map(stamp))
        .collect::<io::Result<Vec<_>>>()?;
    let (session, bad) = d::session(card, mtime(running).ok());
    let mut tail = vec![format!("session|{session}")];
    tail.extend(back.iter().map(|b| format!("backup|{b}")));
    for (f, name, summary) in wallet::wallet(cards) {
        let t = if wallet::used_up(&cards.join(&f)) {
            "|transferred"
        } else {
            ""
        };
        tail.push(format!(
            "wallet|{f}|{}|{summary}{t}",
            name.as_deref().unwrap_or("-")
        ));
    }
    let with_tail = |first: String| Ok([vec![first], tail.clone()].concat());
    if session == "none" {
        return with_tail(
            "state|none|no club card in the slot yet - put one in with the CARD key".into(),
        );
    }
    let c = match d::read_card(card) {
        Ok(c) => c,
        Err(e) => return with_tail(format!("state|unreadable|{}", ascii_name(&e))),
    };
    if c.is_new() {
        return with_tail(
            "state|new|a new card - no club made yet (put it in and press START)".into(),
        );
    }
    let (a, b, chosen, verdict) = d::game_choice(&c);
    let Some(chosen) = chosen else {
        return with_tail(format!(
            "state|unreadable|the game would refuse this card: {}",
            ascii_name(verdict)
        ));
    };
    let dd = Decoded::of(c.copy(Some(chosen)));
    let one = |g, n| dd.one(g, n);
    let lg = d::league(&dd);
    let name_of = |g, n| ascii_name(&d::text(&dd.get(g, 0, n)));
    let or_dash = |s: String| if s.is_empty() { "-".to_owned() } else { s };
    let mut lines = vec![
        "state|ok".to_owned(),
        "head|CLUB".into(),
        format!(
            "row|CLUB|{}  (founded {})|w",
            or_dash(name_of("Club", "CLUB_NAME")),
            d::ymd8(one("Club", "CLUB_FOUNDATION"))
        ),
        format!(
            "row|MANAGER|{}  (born {})|w",
            or_dash(name_of("Coach", "COACH_NAME")),
            d::ymd8(one("Coach", "COACH_BIRTHDAY"))
        ),
    ];
    let left = one("Coach", "COACH_LAST_TERM");
    let low = wallet::counter(card).unwrap_or(0xFFFF) & 0xFF; // 0 is a real counter (used up)
    lines.push(if low == 0 {
        "row|CONTRACT|transferred - the game will not play this card again|r".into()
    } else if low == 1 || left == 0 {
        "row|CONTRACT|ended - put the card in: the game moves your manager to a new card|o".into()
    } else {
        format!(
            "row|CONTRACT|{left} matches left|{}",
            if left <= 10 { "y" } else { "w" }
        )
    });
    lines.push(format!(
        "row|SALARY|{} a year|w",
        money(one("Coach", "COACH_SALARY"))
    ));
    lines.push(format!(
        "row|LEAGUE|Div {}, leg {}: {} - W{} D{} L{}, {}-{}, {} pts|w",
        lg.division,
        lg.leg,
        ordinal(lg.position_computed),
        lg.win,
        lg.draw,
        lg.lose,
        lg.goals_for,
        lg.goals_against,
        lg.points
    ));
    lines.push(format!(
        "row|ALL MATCHES|W{} D{} L{} - best win streak {}|w",
        one("Club", "CLUB_RECORD_WIN"),
        one("Club", "CLUB_RECORD_DRAW"),
        one("Club", "CLUB_RECORD_LOSE"),
        one("Club", "CLUB_RECORD_MAX_SERIES_WIN")
    ));
    lines.push(format!(
        "row|PRIZE MONEY|{}|w",
        money(one("Club", "CLUB_GET_PRIZE"))
    ));
    lines.push(format!(
        "row|TRADE RIGHTS|{}  ({} left)|w",
        one("SystemInfo", "CLUB_TRADE_RIGHT_NUM"),
        one("SystemInfo", "CLUB_TRADE_REMAINDER_NUM")
    ));
    lines.push(format!(
        "row|LAST PLAYED|{}|w",
        d::ymd6(one("SystemInfo", "SYSTEM_LAST_PLAY_DAY"))
    ));
    lines.push("head|CARD HEALTH".into());
    lines.push(format!(
        "row|SESSION|{}",
        match session {
            "open" => "OPEN - a match is on: do not stop the game|o".to_owned(),
            "cut" => "the last session did not end normally|y".into(),
            "closed" => "closed - safe to stop|g".into(),
            s => format!("{s}|w"),
        }
    ));
    lines.push(format!(
        "row|BAD ENDINGS|{}",
        match bad.unwrap_or(0) {
            0 => "0|g".to_owned(),
            1 => "1 - at 2, trade rights are lost|y".into(),
            n => format!("{n} - trade rights lost; 5th costs money|r"),
        }
    ));
    lines.push(
        if a.good && b.good && c.copy(Some('A')) == c.copy(Some('B')) {
            "row|SAVED COPIES|both good, the same|g".into()
        } else if chosen == 'A' {
            format!(
                "row|SAVED COPIES|copy A good, copy B {}|y",
                if b.good { "different" } else { "bad" }
            )
        } else {
            "row|SAVED COPIES|copy A bad - the game reads copy B|y".into()
        },
    );
    lines.push(format!("row|CARD SAVED|{}|w", stamp(mtime(card)?)));
    lines.push(match back.first() {
        Some(newest) => format!("row|BACKUPS|{} kept, newest {newest}|w", found.len()),
        None => "row|BACKUPS|none yet - made when a session starts|d".into(),
    });
    let names = load_names(catalogue);
    let mut squad = Vec::new();
    for i in 0..16 {
        let p = |n| dd.get("Player", i, n)[0];
        let no = p("PLAYER_CARD_NO");
        if no == 0 {
            continue;
        }
        let (name, pos) = names
            .get(&no)
            .cloned()
            .unwrap_or_else(|| (format!("card {no}"), "?".into()));
        // injured when PLAYER_INJURY_PART is set (checked in the game); the part, degree and healing are not
        let injury = if p("PLAYER_INJURY_PART") != 0 {
            "injured"
        } else {
            "-"
        };
        let order = ["GK", "DF", "MF", "FW"]
            .iter()
            .position(|&x| x == pos)
            .unwrap_or(4);
        squad.push((
            order,
            i,
            format!(
                "squad|{}|{}|{}|{}|{}|{}|{}|{injury}",
                p("PLAYER_BACK_NUMBER"),
                if name.is_empty() {
                    format!("card {no}")
                } else {
                    name
                },
                if pos.is_empty() { "?" } else { &pos },
                p("PLAYER_PARTICIPATE_NUM"),
                p("PLAYER_OFFICIAL_GOAL"),
                p("PLAYER_OFFICIAL_ASSIST"),
                p("PLAYER_CONDITION")
            ),
        ));
    }
    squad.sort(); // GK, DF, MF, FW, then card order: the game's
    lines.extend(squad.into_iter().map(|s| s.2));
    lines.extend(tail);
    Ok(lines)
}

/// The view whole: a temporary file, then one swap (the panel never reads half of it) - view.write.
pub fn write(lines: &[String], path: &Path) -> io::Result<()> {
    let text = format!(
        "# Forza-WCCF 2010-11 - the club card as the game reads it, for the CLUB CARD panel in seat 1. Written by\n\
         # Launcher.exe with decode.py whenever the card changes. Read-only, never edit.\n{}\n",
        lines.join("\n")
    );
    let ascii: String = text
        .chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, ascii)?;
    for attempt in 0..25 {
        // a scanner or indexer may hold a fresh file a moment: up to 0.5 s, as the card reader does for the card
        match fs::rename(&tmp, path) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < 24 => {
                sleep(Duration::from_millis(20));
            }
            r => return r,
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_ascii() {
        assert_eq!(ascii_name("ＡＢＣ"), "ABC");
        assert_eq!(ascii_name("カカ"), "KAKA");
        assert_eq!(ascii_name("キャプテン"), "KYAPUTEN");
        assert_eq!(ascii_name("シュート"), "SHUUTO");
        assert_eq!(ascii_name("サッカー"), "SAKKAA");
        assert_eq!(ascii_name("ファン"), "FAN");
        assert_eq!(ascii_name("ｱｲﾝ"), "AIN");
        assert_eq!(ascii_name("ちゃん"), "CHAN");
        assert_eq!(ascii_name(" Pelé|Ø "), "Pele?O");
        assert_eq!(ascii_name("漢"), "?");
    }

    #[test]
    fn money_and_ordinals() {
        assert_eq!(money(2075), "$207,500");
        assert_eq!(money(35970), "$3,597,000");
        assert_eq!(money(0), "$0");
        let o: Vec<_> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 111, 101]
            .map(ordinal)
            .into();
        assert_eq!(
            o,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "111th",
                "101st"
            ]
        );
    }
}
