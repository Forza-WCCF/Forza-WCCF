//! A key list as a person gets it: an HTML table with `Game ID`, `AES Key` and `AES IV` columns, or a ZIP holding one.
//! Only the rows of the disc's game id count; tables without those columns are ignored.
use std::io::{Cursor, Read};

pub type Pair = ([u8; 16], [u8; 16]);

/// The (key, iv) pairs of `game_id`, duplicates removed, in table order. `None`: the input is no key table at all.
pub fn pairs(raw: &[u8], game_id: &str) -> Option<Vec<Pair>> {
    let mut found = None;
    let mut add = |html: &str| {
        if let Some(p) = table_pairs(html, game_id) {
            let all: &mut Vec<Pair> = found.get_or_insert_with(Vec::new);
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

/// `None`: no table here has the three columns. `Some`: the rows of `game_id` (possibly none).
fn table_pairs(html: &str, game_id: &str) -> Option<Vec<Pair>> {
    let dom = tl::parse(html, tl::ParserOptions::default()).ok()?;
    let p = dom.parser();
    let mut cols: Option<[usize; 3]> = None; // Game ID, AES Key, AES IV of the table being read
    let mut saw = false;
    let mut out = vec![];
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
            cols = [col("Game ID"), col("AES Key"), col("AES IV")]
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .map(|v| [v[0], v[1], v[2]]);
            saw |= cols.is_some();
            continue;
        }
        let Some([g, k, v]) = cols else { continue };
        let get = |i: usize| cells.get(i).map(|(_, c)| c.as_str());
        if get(g).is_some_and(|c| c.eq_ignore_ascii_case(game_id))
            && let (Some(key), Some(iv)) = (get(k).and_then(hex16), get(v).and_then(hex16))
        {
            out.push((key, iv));
        }
    }
    saw.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const K1: &str = "00112233445566778899aabbccddeeff";
    const K2: &str = "ffeeddccbbaa99887766554433221100";
    const IV: &str = "0f0e0d0c0b0a09080706050403020100";

    pub fn table(rows: &[(&str, &str, &str)]) -> String {
        let mut s = String::from(
            "<html><body><table><tr><th>Game ID</th><th>Region</th><th>AES Key</th><th>AES IV</th></tr>",
        );
        for (g, k, i) in rows {
            s += &format!("<tr><td>{g}</td><td>JPN</td><td>{k}</td><td>{i}</td></tr>");
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
        let p = pairs(html.as_bytes(), "SDDY").unwrap();
        assert_eq!(
            p,
            [
                (hex16(K1).unwrap(), hex16(IV).unwrap()),
                (hex16(K2).unwrap(), hex16(IV).unwrap())
            ]
        );
        assert_eq!(pairs(html.as_bytes(), "XXXX").unwrap(), []); // a table, but no entry
        assert!(pairs(b"0011223344", "SDDY").is_none());
        assert!(
            pairs(
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
        assert_eq!(pairs(&raw, "SDDY").unwrap().len(), 1);
    }
}
