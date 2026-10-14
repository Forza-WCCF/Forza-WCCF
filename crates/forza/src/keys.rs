//! The keys and controller controls of keys.txt, shared by the key driver (`launcher::keys`) and the panel's KEYS page:
//! which keys an action may have, the controls' numbers and words, their names on screen, the actions in the KEYS
//! page's order with their defaults, and keys.txt as the KEYS page writes it (`wccfpanel.c` `keys_save_locked`).

/// Keys an action may have: letters, digits, the number pad, F1-F9 and F12-F24, arrows and the keys around them, Enter
/// Space Tab Backspace Shift Ctrl Pause and the punctuation keys. Never Esc (it closes the panels), F10 (Windows' menu
/// key froze the game window), F11 (the window's size), Alt, the Windows keys, the lock keys or a mouse button.
pub fn is_allowed(vk: i128) -> bool {
    matches!(vk,
        0x08 | 0x09 | 0x0D | 0x10 | 0x11 | 0x13 | 0x20..=0x28 | 0x2D | 0x2E | 0xE2
        | 0x30..=0x39 | 0x41..=0x5A | 0x60..=0x78 | 0x7B..=0x87 | 0xBA..=0xC0 | 0xDB..=0xDF)
}

/// Controller controls are numbers above every key code, so keys and controls share one set: 0x100 + n = button n
/// (1-32); 0x200 + 2 * axis + (1 for +) = a stick or axis direction (axes X Y Z R U V; up is Y-); 0x300 + n = the
/// hat's up/right/down/left. The KEYS panel (`wccfpanel.dll`) reads controllers the same way: keep these numbers, the
/// words and the rest rule of `launcher::keys::PadDevice` equal in both.
pub const PAD_BUTTON: u32 = 0x100;
pub const PAD_AXIS: u32 = 0x200;
pub const PAD_HAT: u32 = 0x300;
const AXES: &str = "XYZRUV";
const HATS: [&str; 4] = ["UP", "RIGHT", "DOWN", "LEFT"];

/// A control's word in `keys.txt` ("B3", "X+", "HAT UP").
pub fn pad_token(code: u32) -> Option<String> {
    if (PAD_BUTTON + 1..=PAD_BUTTON + 32).contains(&code) {
        return Some(format!("B{}", code - PAD_BUTTON));
    }
    if (PAD_AXIS..PAD_AXIS + 12).contains(&code) {
        let (axis, plus) = ((code - PAD_AXIS) / 2, (code - PAD_AXIS) % 2);
        return Some(format!(
            "{}{}",
            AXES.as_bytes()[axis as usize] as char,
            if plus == 1 { "+" } else { "-" }
        ));
    }
    if (PAD_HAT..PAD_HAT + 4).contains(&code) {
        return Some(format!("HAT {}", HATS[(code - PAD_HAT) as usize]));
    }
    None
}

/// `keys.txt`'s word -> the control, 0 if it is none (case and spaces do not matter: "hat up" = "HATUP").
pub fn pad_code(word: &str) -> u32 {
    let t: Vec<char> = word
        .split_whitespace()
        .collect::<String>()
        .to_uppercase()
        .chars()
        .collect();
    let s: String = t.iter().collect();
    if let Some(rest) = s.strip_prefix("HAT")
        && let Some(i) = HATS.iter().position(|h| *h == rest)
    {
        return PAD_HAT + i as u32;
    }
    if t.len() == 2
        && (t[1] == '+' || t[1] == '-')
        && let Some(axis) = AXES.chars().position(|c| c == t[0])
    {
        return PAD_AXIS + 2 * axis as u32 + u32::from(t[1] == '+');
    }
    let n: String = t.iter().skip(1).collect();
    if t.first() == Some(&'B')
        && !n.is_empty()
        && n.len() <= 4
        && n.bytes().all(|b| b.is_ascii_digit())
        && let Ok(v) = n.parse::<u32>()
        && (1..=32).contains(&v)
    {
        return PAD_BUTTON + v;
    }
    0
}

/// A control's name on screen and in the log.
pub fn pad_label(code: u32) -> String {
    if code == 0 {
        return "(none)".into();
    }
    if (PAD_BUTTON + 1..=PAD_BUTTON + 32).contains(&code) {
        return format!("Button {}", code - PAD_BUTTON);
    }
    if (PAD_AXIS..PAD_AXIS + 12).contains(&code) {
        let (axis, plus) = ((code - PAD_AXIS) / 2, ((code - PAD_AXIS) % 2) as usize);
        return match axis {
            0 => ["Stick Left", "Stick Right"][plus].into(),
            1 => ["Stick Up", "Stick Down"][plus].into(),
            _ => format!(
                "Axis {}{}",
                AXES.as_bytes()[axis as usize] as char,
                ["-", "+"][plus]
            ),
        };
    }
    if (PAD_HAT..PAD_HAT + 4).contains(&code) {
        let h = HATS[(code - PAD_HAT) as usize].to_lowercase();
        let mut c = h.chars();
        let first = c
            .next()
            .map(|f| f.to_uppercase().to_string())
            .unwrap_or_default();
        return format!("D-pad {first}{}", c.as_str());
    }
    format!("0x{code:X}")
}

/// A key's name for the log: letters and digits as they are, else the common names.
pub fn key_label(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        _ => match vk {
            0x08 => "Backspace",
            0x09 => "Tab",
            0x0D => "Enter",
            0x10 => "Shift",
            0x11 => "Ctrl",
            0x13 => "Pause",
            0x20 => "Space",
            0x21 => "PgUp",
            0x22 => "PgDn",
            0x23 => "End",
            0x24 => "Home",
            0x25 => "Left",
            0x26 => "Up",
            0x27 => "Right",
            0x28 => "Down",
            0x2D => "Insert",
            0x2E => "Delete",
            0x6A => "Num*",
            0x6B => "Num+",
            0x6C => "NumSep",
            0x6D => "Num-",
            0x6E => "Num.",
            0x6F => "Num/",
            0xBA => ";",
            0xBB => "=",
            0xBC => ",",
            0xBD => "-",
            0xBE => ".",
            0xBF => "/",
            0xC0 => "`",
            0xDB => "[",
            0xDC => "\\",
            0xDD => "]",
            0xDE => "'",
            0xDF => "OEM8",
            0xE2 => "<>",
            0 => "(none)",
            _ => return format!("0x{vk:02X}"),
        }
        .into(),
    }
}

// ---- the KEYS page's actions and keys.txt

/// The 16 actions by their names in keys.txt, in the KEYS page's order (`wccfpanel.c` `g_kact`): the buttons, CARD and
/// COIN, the tactics, the operator's.
pub const NAMES: [&str; 16] = [
    "START",
    "PRESS",
    "SHOOT",
    "KEEPER",
    "DATA",
    "KEYPLAYER",
    "CARD",
    "COIN",
    "UP",
    "DOWN",
    "LEFT",
    "RIGHT",
    "TEST",
    "SERVICE",
    "SENSOR2",
    "SENSOR3",
];

/// The default keys in [`NAMES`] order. SW1 and SW2 (SENSOR2/3) have none in 2010-11: on the original's club-card
/// recovery screen SW1 overwrites the card with its backup; the 2017-18 kit gives them 8 and 9.
pub fn defaults(e1718: bool) -> [u32; 16] {
    let sw = if e1718 { [0x38, 0x39] } else { [0, 0] };
    [
        0x0D, 0x58, 0x43, 0x42, 0x53, 0x44, 0x49, 0x35, 0x26, 0x28, 0x25, 0x27, 0x70, 0x71, sw[0],
        sw[1],
    ]
}

/// keys.txt as the KEYS page writes it, byte for byte as `wccfpanel.c` `keys_save_locked` (each edition's own first
/// lines): every action's key in [`NAMES`] order ("# NAME has no key" for none), then the controller controls.
pub fn keys_text(e1718: bool, keys: &[u32; 16], pads: &[u32; 16]) -> String {
    let driver = if e1718 {
        "_keys_seat1.py"
    } else {
        "keyboard.py"
    };
    let mut s = format!(
        "# WCCF 2010-11 kit - which key and which controller control does what. Written by the KEYS panel\n# in seat \
         1's window, read by the key driver ({driver}). Delete this file to get the default keys back.\n"
    );
    for (name, vk) in NAMES.iter().zip(keys) {
        s += &if *vk != 0 {
            format!("{name}=0x{vk:02X}\n")
        } else {
            format!("# {name} has no key\n")
        };
    }
    s += "# controllers (any game controller): B1-B32 its buttons, X- X+ Y- Y+ the stick (left right up down),\n# Z R \
          U V its other axes (+ or -), HAT UP / RIGHT / DOWN / LEFT its d-pad\n";
    for (name, pad) in NAMES.iter().zip(pads) {
        if let Some(w) = pad_token(*pad) {
            s += &format!("PAD {name}={w}\n");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The C panel's own output for these bindings (2010-11 header, SHOOT moved to G, SW1/SW2 without keys, two
    /// controller controls).
    #[test]
    fn keys_txt_as_the_c_panel_writes_it() {
        let mut keys = defaults(false);
        keys[2] = 0x47;
        let mut pads = [0; 16];
        pads[2] = PAD_BUTTON + 3;
        pads[8] = PAD_HAT;
        assert_eq!(
            keys_text(false, &keys, &pads).as_bytes(),
            b"# WCCF 2010-11 kit - which key and which controller control does what. Written by the KEYS panel\n\
# in seat 1's window, read by the key driver (keyboard.py). Delete this file to get the default keys back.\n\
START=0x0D\nPRESS=0x58\nSHOOT=0x47\nKEEPER=0x42\nDATA=0x53\nKEYPLAYER=0x44\nCARD=0x49\nCOIN=0x35\nUP=0x26\n\
DOWN=0x28\nLEFT=0x25\nRIGHT=0x27\nTEST=0x70\nSERVICE=0x71\n# SENSOR2 has no key\n# SENSOR3 has no key\n\
# controllers (any game controller): B1-B32 its buttons, X- X+ Y- Y+ the stick (left right up down),\n\
# Z R U V its other axes (+ or -), HAT UP / RIGHT / DOWN / LEFT its d-pad\n\
PAD SHOOT=B3\nPAD UP=HAT UP\n"
        );
        assert!(keys_text(true, &defaults(true), &[0; 16]).contains("(_keys_seat1.py)"));
        assert!(
            keys_text(true, &defaults(true), &[0; 16]).contains("\nSENSOR2=0x38\nSENSOR3=0x39\n")
        );
    }
}
