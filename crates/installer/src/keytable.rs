//! A key list as a person gets it: an HTML table with a `Game ID` column and `Keyfile`, `AES Key` or `AES IV` columns, or a
//! ZIP holding one. Only the rows of the disc's game id count; tables without those columns are ignored.
use std::io::{Cursor, Read};

/// The columns tried, in this order: the disc's keyfile is the `Keyfile` value; the AES columns are a fallback.
const KEY_COLUMNS: [&str; 3] = ["Keyfile", "AES Key", "AES IV"];

/// The 16-byte values of `game_id`'s rows (all `Keyfile` values, then `AES Key`, then `AES IV`), duplicates removed.
/// `None`: the input is no key table at all.
pub fn candidates(raw: &[u8], game_id: &str) -> Option<Vec<[u8; 16]>> {
    let mut found = None;
    let mut add = |html: &str| {
        if let Some(p) = table_values(html, game_id) {
            let all: &mut Vec<[u8; 16]> = found.get_or_insert_with(Vec::new);
            for x in p {
                if !all.contains(&x) {
                    all.push(x);
                }
            }
        }
    };
    if raw.starts_with(b"PK\x03\x04") {
        let mut zip = zip::ZipArchive::new(Cursor::new(raw)).ok()?;
        for i in 0..zip.len() {
            let Ok(f) = zip.by_index(i) else { continue };
            let name = f.name().map(|n| n.to_ascii_lowercase()).unwrap_or_default();
            if name.ends_with(".html") || name.ends_with(".htm") {
                let mut buf = Vec::new();
                if f.take(256 << 20).read_to_end(&mut buf).is_ok() {
                    add(&String::from_utf8_lossy(&buf));
                }
            }
        }
    } else if raw.contains(&b'<') {
        add(&String::from_utf8_lossy(raw));
    }
    found
}

fn hex16(s: &str) -> Option<[u8; 16]> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// `None`: no table here has `Game ID` and a key column. `Some`: the values of `game_id`'s rows (possibly none).
fn table_values(html: &str, game_id: &str) -> Option<Vec<[u8; 16]>> {
    let dom = tl::parse(html, tl::ParserOptions::default()).ok()?;
    let p = dom.parser();
    // Game ID's column, and each KEY_COLUMNS column of the table being read
    let mut cols: Option<(usize, [Option<usize>; 3])> = None;
    let mut saw = false;
    let mut by_col: [Vec<[u8; 16]>; 3] = Default::default();
    for node in dom.nodes() {
        let Some(tr) = node.as_tag().filter(|t| t.name().as_bytes() == b"tr") else {
            continue;
        };
        let cells: Vec<(bool, String)> = tr
            .children()
            .top()
            .iter()
            .filter_map(|h| h.get(p)?.as_tag())
            .filter(|t| matches!(t.name().as_bytes(), b"td" | b"th"))
            .map(|t| {
                let text = t.inner_text(p).replace('\u{a0}', " ");
                (t.name().as_bytes() == b"th", text.trim().to_owned())
            })
            .collect();
        let col = |name: &str| cells.iter().position(|(_, c)| c.eq_ignore_ascii_case(name));
        if cells
            .iter()
            .any(|(th, c)| *th || c.eq_ignore_ascii_case("Game ID"))
        {
            let keys = KEY_COLUMNS.map(col);
            cols = col("Game ID")
                .filter(|_| keys.iter().any(Option::is_some))
                .map(|g| (g, keys));
            saw |= cols.is_some();
            continue;
        }
        let Some((g, keys)) = cols else { continue };
        let get = |i: usize| cells.get(i).map(|(_, c)| c.as_str());
        if get(g).is_some_and(|c| c.eq_ignore_ascii_case(game_id)) {
            for (bucket, k) in by_col.iter_mut().zip(keys) {
                bucket.extend(k.and_then(get).and_then(hex16));
            }
        }
    }
    saw.then(|| by_col.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const K1: &str = "00112233445566778899aabbccddeeff";
    const K2: &str = "ffeeddccbbaa99887766554433221100";
    const IV: &str = "0f0e0d0c0b0a09080706050403020100";
    const KF: &str = "a0a1a2a3a4a5a6a7a8a9aaabacadaeaf";

    pub fn table(rows: &[(&str, &str, &str)]) -> String {
        let mut s = String::from(
            "<html><body><table><tr><th>Game ID</th><th>Region</th><th>AES Key</th><th>AES IV</th><th>Keyfile</th></tr>",
        );
        for (g, k, i) in rows {
            s += &format!("<tr><td>{g}</td><td>JPN</td><td>{k}</td><td>{i}</td><td>{KF}</td></tr>");
        }
        s + "</table><table><tr><th>Game ID</th><th>Game Key</th><th>Game IV</th></tr>\
            <tr><td>SDDY</td><td>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</td><td>bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</td></tr></table></body></html>"
    }

    #[test]
    fn rows_of_the_game_id_only_without_duplicates() {
        let html = table(&[
            ("SDDY", K1, IV),
            ("SBWG", K2, IV),
            ("SDDY", K2, IV),
            ("SDDY", K1, IV),
        ]);
        let c = candidates(html.as_bytes(), "SDDY").unwrap();
        let h = |s| hex16(s).unwrap();
        assert_eq!(c, [h(KF), h(K1), h(K2), h(IV)]); // the keyfile first
        assert!(candidates(html.as_bytes(), "XXXX").unwrap().is_empty()); // a table, but no entry
        assert!(candidates(b"0011223344", "SDDY").is_none());
        assert!(
            candidates(
                b"<table><tr><th>Game ID</th><th>Game Key</th></tr></table>",
                "SDDY"
            )
            .is_none()
        );
    }

    #[test]
    fn zip_with_the_table() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("other.html", o).unwrap();
        z.write_all(b"<table><tr><th>Game ID</th><th>Game Key</th></tr></table>")
            .unwrap();
        z.start_file("dir/Keys.HTML", o).unwrap();
        z.write_all(table(&[("SDDY", K1, IV)]).as_bytes()).unwrap();
        let raw = z.finish().unwrap().into_inner();
        assert_eq!(candidates(&raw, "SDDY").unwrap().len(), 3);
    }
}
