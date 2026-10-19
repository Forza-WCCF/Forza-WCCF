//! The desk's `LOBBY edition` answer: one JSON line listing the shops and VS rooms the Launcher's lobby view shows
//! (ROADMAP, "Online lobby view"). Wire types only: no socket, no polling. Ids are random per server run; the answer
//! holds no addresses, card numbers or names of people.

use serde::{Deserialize, Serialize};

/// The wire version this build reads and writes.
pub const VERSION: u32 = 1;

/// A shop's phase, as the desk reads it from the control's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    InMatch,
    Full,
    Closed,
}

/// One shop: the 8 seats of one control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shop {
    pub id: String,
    /// The operator's setting, up to 24 characters.
    pub name: String,
    pub seats: u8,
    pub max: u8,
    pub state: State,
    /// Seconds to the next ENTRY, when the desk knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_entry: Option<u32>,
    /// `en`, `it` or `ja`, declared by the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// One VS room: a one-PC shop waiting for, or holding, its opponent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VsRoom {
    pub id: String,
    /// The four-letter code players read out.
    pub code: String,
    pub players: u8,
    pub max: u8,
    /// The game's VS kind number (243 entries are kind 2).
    pub kind: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// The whole answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lobby {
    pub v: u32,
    pub edition: String,
    /// Seconds since the Unix epoch, when the desk built the answer.
    pub at: u64,
    #[serde(default)]
    pub shops: Vec<Shop>,
    #[serde(default)]
    pub vs: Vec<VsRoom>,
}

impl Lobby {
    /// Reads one answer line. A version this build does not know is an error, not a guess.
    pub fn parse(line: &str) -> Result<Lobby, String> {
        let l: Lobby = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
        if l.v != VERSION {
            return Err(format!(
                "lobby version {} (this build reads {VERSION})",
                l.v
            ));
        }
        Ok(l)
    }

    /// The answer as one line, newline included.
    pub fn line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("plain data serializes");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"v":1,"edition":"2017-18","at":1792400000,"shops":[{"id":"k3f9q2","name":"FORZA the game server","seats":3,"max":8,"state":"open","next_entry":32,"lang":"it"},{"id":"m1","name":"MILANO CUP","seats":5,"max":8,"state":"in_match"}],"vs":[{"id":"r7x1","code":"K7QM","players":1,"max":2,"kind":2}]}"#;

    #[test]
    fn parses_and_round_trips() {
        let l = Lobby::parse(SAMPLE).unwrap();
        assert_eq!(l.shops.len(), 2);
        assert_eq!(l.shops[0].state, State::Open);
        assert_eq!(l.shops[1].state, State::InMatch);
        assert_eq!(l.shops[1].next_entry, None);
        assert_eq!(l.vs[0].code, "K7QM");
        assert_eq!(l.line(), format!("{SAMPLE}\n"));
        assert_eq!(Lobby::parse(&l.line()).unwrap(), l);
    }

    #[test]
    fn refuses_other_versions_and_junk() {
        assert!(Lobby::parse(&SAMPLE.replace("\"v\":1", "\"v\":2")).is_err());
        assert!(Lobby::parse("ERR slow").is_err());
    }
}
