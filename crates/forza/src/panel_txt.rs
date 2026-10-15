//! data\panel.txt as the in-game SETTINGS page reads and writes it, by `wccfpanel.c` `set_load_locked` and
//! `set_save_locked` (play.py's read_panel rules: "name=value", "#" starts a note): where the next plain start plays,
//! the game's text asked for, the club card chosen, the layout, the card hover delay, automatic reports. The file is
//! written whole from these choices, so lines the panel does not know are not kept (as the C panel).

/// Four numbers 0-255 with dots, only the digits 0-9, as play.py's valid_ipv4 (the game takes no names).
pub fn valid_address(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            (1..=3).contains(&p.len())
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// The choices in data\panel.txt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelFile {
    /// play_on=online (else this_pc).
    pub online: bool,
    /// server=, as the file holds it (checked before use).
    pub server: String,
    /// english=on|it|off: a change of the game's text asked for the next start (the run takes the line out).
    pub english: Option<String>,
    /// card=: the CLUB CARD page's choice for the next start ("" none, "new").
    pub card: String,
    /// card_fix=bad_endings: CLEAR BAD ENDINGS at the next start.
    pub card_fix: bool,
    /// layout=compact.
    pub compact: bool,
    /// hover_delay_ms=: 0-5000; missing, not a number or out of range: 1000.
    pub hover_ms: u32,
    /// auto_logs=0: no automatic crash report.
    pub auto_logs_off: bool,
}

impl Default for PanelFile {
    fn default() -> Self {
        PanelFile {
            online: false,
            server: String::new(),
            english: None,
            card: String::new(),
            card_fix: false,
            compact: false,
            hover_ms: 1000,
            auto_logs_off: false,
        }
    }
}

impl PanelFile {
    pub fn parse(raw: &[u8]) -> PanelFile {
        let mut f = PanelFile::default();
        for line in String::from_utf8_lossy(raw).lines() {
            let Some((name, val)) = line.split('#').next().and_then(|l| l.split_once('=')) else {
                continue;
            };
            let (name, val) = (name.trim().to_ascii_lowercase(), val.trim());
            let is = |w: &str| val.eq_ignore_ascii_case(w);
            match name.as_str() {
                "play_on" => f.online = is("online"),
                "server" => f.server = val.chars().take(63).collect(),
                "english" => {
                    f.english = ["on", "it", "off"]
                        .into_iter()
                        .find(|w| is(w))
                        .map(str::to_owned);
                }
                "card" => f.card = val.chars().take(63).collect(),
                "card_fix" => f.card_fix = is("bad_endings"),
                "layout" => f.compact = is("compact"),
                "hover_delay_ms" => {
                    f.hover_ms = val
                        .parse::<i64>()
                        .ok()
                        .filter(|v| (0..=5000).contains(v))
                        .unwrap_or(1000) as u32;
                }
                "auto_logs" => f.auto_logs_off = val == "0",
                _ => {}
            }
        }
        f
    }

    /// The file, byte for byte as `wccfpanel.c` writes it (2017-18: its own first line; one panel for both
    /// editions now, so its file gains the 2010-11 lines: card_fix, hover_delay_ms, auto_logs).
    pub fn text(&self, e1718: bool) -> String {
        let starter = if e1718 { "PLAY.bat" } else { "Launcher.exe" };
        let mut s = format!(
            "# WCCF 2010-11 kit - written by the SETTINGS panel in seat 1's window, read by {starter} at its next\n# \
             plain start (play.py). english= is a change asked for that start; PLAY takes the line out once it is \
             done.\nplay_on={}\n",
            if self.online { "online" } else { "this_pc" }
        );
        if !self.server.is_empty() {
            s += &format!("server={}\n", self.server);
        }
        if let Some(e) = &self.english {
            s += &format!("english={e}\n");
        }
        if !self.card.is_empty() {
            s += &format!("card={}\n", self.card);
        }
        if self.card_fix {
            s += "card_fix=bad_endings\n";
        }
        if self.compact {
            s += "layout=compact\n";
        }
        s += &format!("hover_delay_ms={}\n", self.hover_ms);
        if self.auto_logs_off {
            s += "auto_logs=0\n";
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wccfpanel.c's own output for these choices, and its reading rules (set_selftest's cases).
    #[test]
    fn panel_txt_as_the_c_panel() {
        let f = PanelFile::parse(
            b"play_on=ONLINE\r\nserver= 1.2.3.4 # mine\nauto_logs=0\nenglish=off\nwhat=ever\n",
        );
        assert_eq!(
            f.text(false).as_bytes(),
            b"# WCCF 2010-11 kit - written by the SETTINGS panel in seat 1's window, read by Launcher.exe at its next\n\
# plain start (play.py). english= is a change asked for that start; PLAY takes the line out once it is done.\n\
play_on=online\nserver=1.2.3.4\nenglish=off\nhover_delay_ms=1000\nauto_logs=0\n"
        );
        let all = PanelFile {
            card: "new".into(),
            card_fix: true,
            compact: true,
            english: Some("it".into()),
            hover_ms: 2000,
            ..PanelFile::default()
        };
        assert_eq!(PanelFile::parse(all.text(true).as_bytes()), all);
        assert!(all.text(true).contains("read by PLAY.bat at its next\n"));
        assert!(
            all.text(false).ends_with("play_on=this_pc\nenglish=it\ncard=new\ncard_fix=bad_endings\nlayout=compact\nhover_delay_ms=2000\n")
        );
        for (line, ms) in [
            ("", 1000),
            ("hover_delay_ms=5000", 5000),
            ("hover_delay_ms=0", 0),
            ("hover_delay_ms=5001", 1000),
            ("hover_delay_ms=abc", 1000),
        ] {
            assert_eq!(PanelFile::parse(line.as_bytes()).hover_ms, ms, "{line}");
        }
    }
}
