//! The virtual serial ports MXHOOK_COM names (`serial` serves them): `COM1=\\.\pipe\wccf_icc_seat1`, several
//! separated by `;`.

pub const MAX: usize = 4;

#[derive(Debug, PartialEq, Eq)]
pub struct Port {
    /// the port the game opens: `COM1`, `COM4`, `mxjvs`
    pub com: String,
    /// the stand-in's named pipe
    pub pipe: String,
    /// the I/O board: overlapped I/O and the JVS sense line. "mxjvs", or any port served by the JVS stand-in's pipe; in
    /// JVS mode 0 (seat 1's patch 0x004D91C9 push 1 -> push 0) the game opens the board as "COM4" (FUN_0082d5b0).
    pub jvs: bool,
}

/// The ports of MXHOOK_COM, at most MAX. An entry without `=`, or with a port name of 16 characters or more, is
/// skipped; an empty entry ends the list.
pub fn parse(spec: &str) -> Vec<Port> {
    if spec.len() >= 1024 {
        return vec![];
    }
    spec.split(';')
        .take_while(|item| !item.is_empty())
        .filter_map(|item| {
            let (com, pipe) = item.split_once('=')?;
            (!com.is_empty() && com.len() < 16).then(|| Port {
                com: com.into(),
                pipe: pipe.into(),
                jvs: com.eq_ignore_ascii_case("mxjvs") || pipe.to_ascii_lowercase().contains("jvs"),
            })
        })
        .take(MAX)
        .collect()
}

/// The port a CreateFileA name opens (`COM1` or `\\.\COM1`), if MXHOOK_COM names it.
pub fn find<'a>(coms: impl IntoIterator<Item = &'a str>, name: &str) -> Option<usize> {
    let name = name.strip_prefix(r"\\.\").unwrap_or(name);
    coms.into_iter()
        .position(|com| com.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_reader_and_the_io_board() {
        let p = parse(
            r"COM1=\\.\pipe\wccf_icc_seat1;COM4=\\.\pipe\wccf_JVS_seat1;x;=y;COM123456789012345=z",
        );
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].com.as_str(), p[0].pipe.as_str(), p[0].jvs),
            ("COM1", r"\\.\pipe\wccf_icc_seat1", false)
        );
        assert!(p[1].jvs, "a pipe with jvs in its name serves the I/O board");
        assert!(parse(r"mxjvs=\\.\pipe\board")[0].jvs);
        let coms = || p.iter().map(|p| p.com.as_str());
        assert_eq!(find(coms(), r"\\.\com4"), Some(1));
        assert_eq!(find(coms(), "COM2"), None);
    }

    #[test]
    fn limits() {
        assert_eq!(parse("A=1;B=2;C=3;D=4;E=5").len(), MAX);
        assert_eq!(parse("A=1;;B=2").len(), 1, "an empty entry ends the list");
        assert!(
            parse(&format!("A={}", "p".repeat(1100))).is_empty(),
            "a value too long for the hook is ignored"
        );
    }
}
