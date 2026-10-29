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
    /// hover_delay_ms=: 0-5000, or off (None: no card preview); missing, not a number or out of range: 500.
    pub hover_ms: Option<u32>,
    /// auto_logs=0: no automatic crash report.
    pub auto_logs_off: bool,
    /// board=window: the formation board in a window of their own (the pop-out board); the game's window keeps the
    /// pages and the cabinet buttons.
    pub board_window: bool,
    /// board_buttons=on: the pop-out board's window shows the cabinet buttons too.
    pub board_buttons: bool,
    /// board_rect=x,y,w,h,max: where that window was last (its restored place in screen pixels, and maximized).
    pub board_rect: Option<[i32; 5]>,
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
            hover_ms: Some(500),
            auto_logs_off: false,
            board_window: false,
            board_buttons: false,
            board_rect: None,
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
                    // older builds read off as 500
                    f.hover_ms = if is("off") {
                        None
                    } else {
                        Some(
                            val.parse::<i64>()
                                .ok()
                                .filter(|v| (0..=5000).contains(v))
                                .unwrap_or(500) as u32,
                        )
                    };
                }
                "auto_logs" => f.auto_logs_off = val == "0",
                "board" => f.board_window = is("window"),
                "board_buttons" => f.board_buttons = is("on"),
                "board_rect" => {
                    let n: Vec<i32> = val
                        .split(',')
                        .filter_map(|v| v.trim().parse().ok())
                        .collect();
                    f.board_rect = <[i32; 5]>::try_from(n)
                        .ok()
                        .filter(|r| r[2] > 0 && r[3] > 0);
                }
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
            "# Forza-WCCF 2010-11 - written by the SETTINGS panel in seat 1's window, read by {starter} at its next\n# \
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
        match self.hover_ms {
            Some(ms) => s += &format!("hover_delay_ms={ms}\n"),
            None => s += "hover_delay_ms=off\n",
        }
        if self.auto_logs_off {
            s += "auto_logs=0\n";
        }
        if self.board_window {
            s += "board=window\n";
        }
        if self.board_buttons {
            s += "board_buttons=on\n";
        }
        if let Some([x, y, w, h, max]) = self.board_rect {
            s += &format!("board_rect={x},{y},{w},{h},{max}\n");
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
            b"# Forza-WCCF 2010-11 - written by the SETTINGS panel in seat 1's window, read by Launcher.exe at its next\n\
# plain start (play.py). english= is a change asked for that start; PLAY takes the line out once it is done.\n\
play_on=online\nserver=1.2.3.4\nenglish=off\nhover_delay_ms=500\nauto_logs=0\n"
        );
        let all = PanelFile {
            card: "new".into(),
            card_fix: true,
            compact: true,
            english: Some("it".into()),
            hover_ms: Some(2000),
            board_window: true,
            board_buttons: true,
            board_rect: Some([-1600, 20, 900, 1400, 0]),
            ..PanelFile::default()
        };
        assert_eq!(PanelFile::parse(all.text(true).as_bytes()), all);
        assert!(all.text(true).contains("read by PLAY.bat at its next\n"));
        assert!(
            all.text(false).ends_with("play_on=this_pc\nenglish=it\ncard=new\ncard_fix=bad_endings\nlayout=compact\nhover_delay_ms=2000\nboard=window\nboard_buttons=on\nboard_rect=-1600,20,900,1400,0\n")
        );
        for (line, ms) in [
            ("", Some(500)),
            ("hover_delay_ms=5000", Some(5000)),
            ("hover_delay_ms=0", Some(0)),
            ("hover_delay_ms=5001", Some(500)),
            ("hover_delay_ms=abc", Some(500)),
            ("hover_delay_ms=OFF", None),
        ] {
            assert_eq!(PanelFile::parse(line.as_bytes()).hover_ms, ms, "{line}");
        }
        let off = PanelFile {
            hover_ms: None,
            ..PanelFile::default()
        };
        assert!(off.text(true).contains("\nhover_delay_ms=off\n"));
        assert_eq!(PanelFile::parse(off.text(true).as_bytes()), off);
        // a board window place that is not five numbers, or has no size, is dropped
        for bad in [
            "board_rect=1,2,3",
            "board_rect=1,2,0,4,0",
            "board_rect=a,b,c,d,e",
        ] {
            assert_eq!(PanelFile::parse(bad.as_bytes()).board_rect, None, "{bad}");
        }
    }
}
