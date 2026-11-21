// ---- scrub ----
use super::*;

/// The publisher's name (and its katakana), built from bytes so this file does not spell it.
pub(crate) fn publisher() -> [String; 2] {
    [
        String::from_utf8(vec![0x73, 0x65, 0x67, 0x61]).unwrap(),
        String::from_utf8(vec![0xE3, 0x82, 0xBB, 0xE3, 0x82, 0xAC]).unwrap(),
    ]
}

/// The line with every case-insensitive occurrence of the publisher's name replaced by `<P>`: the form xtask/scrub-allow.txt
/// stores, so the allow-list does not spell the name either.
pub(crate) fn mask(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        if let Some(p) = publisher()
            .iter()
            .find(|p| lower[i..].starts_with(p.as_str()))
        {
            out.push_str("<P>");
            i += p.len();
        } else {
            let c = line[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// Whether `text` names the publisher outside words that only contain the name: the 2017-18 secretary's surname
/// Hasegawa does ("Ha<P>wa").
pub(crate) fn names_publisher(text: &str) -> bool {
    mask(&text.to_ascii_lowercase().replace("hasegawa", "")).contains("<P>")
}

/// Every tracked text file (not binaries): no line names the publisher except the class-C lines of
/// xtask/scrub-allow.txt (the JVS board ID, the card reader's AES key, keychip strings, third-party tool and host names, the logowin argument
/// name in comments, an in-game club name, and the docs quoting them), and no line matches ~/.config/forza-wccf/scrub.txt
/// (personal data, kept out of the repository). Matches are compared by line text, not by file.
pub(crate) fn scrub(root: &Path, failed: &mut Vec<String>) -> Result<()> {
    let sh = Shell::new()?;
    sh.change_dir(root);
    let allow: HashSet<&str> = include_str!("../scrub-allow.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let [name, kana] = publisher();
    let mut bad = vec![];
    let own = cmd!(sh, "git grep -n -I -i -e {name} -e {kana} -- .")
        .ignore_status()
        .read()?;
    bad.extend(
        own.lines()
            .filter(|l| {
                let text = l.splitn(3, ':').nth(2).unwrap_or("").trim();
                names_publisher(text) && !allow.contains(mask(text).as_str())
            })
            .map(str::to_owned),
    );
    // originals and derived data never get committed: no tracked binary, disc image, IDA database or log
    // (nothing is allowed today; a legitimate fixture would be listed here)
    let tracked = cmd!(
        sh,
        "git ls-files -- *.exe *.dll *.iso *.chd *.bin *.i64 *.idb *.id0 *.til *.nam *.log :!vendor"
    )
    .read()?;
    bad.extend(
        tracked
            .lines()
            .map(|l| format!("tracked binary or log: {l}")),
    );
    if let Some(list) = std::env::home_dir()
        .map(|h| h.join(".config/forza-wccf/scrub.txt"))
        .filter(|p| p.is_file())
    {
        let pats: Vec<String> = fs::read_to_string(list)?
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .flat_map(|l| ["-e".to_owned(), l.to_owned()])
            .collect();
        let hits = cmd!(sh, "git grep -n -I -i -E {pats...} -- .")
            .ignore_status()
            .read()?;
        bad.extend(hits.lines().map(str::to_owned));
    }
    for l in &bad {
        println!("scrub: {}", l.chars().take(200).collect::<String>());
    }
    if bad.is_empty() {
        println!("scrub: clean");
    } else {
        failed.push(format!("scrub ({} lines)", bad.len()));
    }
    Ok(())
}
