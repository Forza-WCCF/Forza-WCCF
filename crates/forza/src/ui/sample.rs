//! Invented sample data for the gallery and the snapshots: players, clubs and countries that do not exist, with
//! Italian accents and Japanese names so every screen is tried with them.

use super::board::{Aptitude, Board, Card, Position, ROLES, Rarity};
use egui::pos2;

#[allow(clippy::too_many_arguments)] // one sample card per call, every field named in order
fn card(
    number: u32,
    name: &str,
    club: &str,
    country: &str,
    pos: Position,
    rarity: Rarity,
    stats: [u8; 6],
    aptitude: Aptitude,
) -> Card {
    Card {
        number,
        name: name.into(),
        short: String::new(),
        club: club.into(),
        country: country.into(),
        season: "10-11".into(),
        pos,
        rarity,
        stats,
        // TOTAL on a 60-100 scale from the six 1-20 stats
        total: (60 + stats.iter().map(|&s| s as u32).sum::<u32>() * 40 / 120) as u8,
        aptitude,
    }
}

/// Synthetic position ratings (never values from the original files): the named roles, every other role 1.
fn roles(rated: &[(&str, u8)]) -> Aptitude {
    let mut roles = [1; 16];
    for (name, r) in rated {
        let i = ROLES.iter().position(|n| n == name).expect("a role name");
        roles[i] = *r;
    }
    Aptitude { roles }
}

fn keeper() -> Aptitude {
    roles(&[("GK", 10)])
}
fn centre_back() -> Aptitude {
    roles(&[
        ("CB", 10),
        ("STP", 9),
        ("CVR", 8),
        ("SW", 7),
        ("DMF", 4),
        ("SB", 3),
    ])
}
fn full_back() -> Aptitude {
    roles(&[("SB", 9), ("WB", 8), ("SMF", 5), ("CB", 5), ("CVR", 4)])
}
fn box_to_box() -> Aptitude {
    roles(&[
        ("CMF", 10),
        ("DMF", 8),
        ("OH", 8),
        ("OMF", 6),
        ("SMF", 5),
        ("SS", 4),
    ])
}
fn playmaker() -> Aptitude {
    roles(&[("OMF", 10), ("OH", 8), ("CMF", 7), ("SS", 6)])
}
fn holding() -> Aptitude {
    roles(&[("DMF", 10), ("CMF", 7), ("CVR", 6), ("CB", 5)])
}
fn winger() -> Aptitude {
    roles(&[("WG", 10), ("SMF", 8), ("OH", 6), ("SS", 5), ("WB", 4)])
}
fn striker() -> Aptitude {
    roles(&[("CF", 10), ("SS", 8), ("TM", 7), ("WG", 4)])
}
fn target_man() -> Aptitude {
    roles(&[("TM", 10), ("CF", 9), ("SS", 4)])
}

pub fn cards() -> Vec<Card> {
    use Position::*;
    use Rarity::*;
    vec![
        card(
            1,
            "Tomás Arribe",
            "Real Olmedo",
            "Hispania",
            Gk,
            Rare,
            [3, 15, 9, 12, 8, 14],
            keeper(),
        ),
        card(
            2,
            "Niccolò Bàrtoli",
            "Porto Azzurro",
            "Italia",
            Df,
            Legend,
            [9, 19, 14, 15, 13, 16],
            centre_back(),
        ),
        card(
            3,
            "Kōji Asanuma",
            "Kitakaze",
            "日本",
            Df,
            Regular,
            [7, 15, 11, 13, 14, 15],
            centre_back(),
        ),
        card(
            4,
            "Hélder Maçãs",
            "Atlântico",
            "Lusitânia",
            Df,
            Special,
            [11, 16, 13, 12, 16, 14],
            full_back(),
        ),
        card(
            5,
            "Jens Ødegård",
            "Nordhavn",
            "Nordland",
            Df,
            Regular,
            [6, 14, 9, 16, 11, 13],
            centre_back(),
        ),
        card(
            6,
            "Lucà Ferrè",
            "Porto Azzurro",
            "Italia",
            Mf,
            AllTime,
            [17, 13, 20, 13, 15, 17],
            playmaker(),
        ),
        card(
            7,
            "Taiga Hoshino",
            "Kitakaze",
            "日本",
            Mf,
            Rare,
            [14, 11, 17, 10, 16, 18],
            box_to_box(),
        ),
        card(
            8,
            "Émile Fourcade",
            "Lyonnaise",
            "Gallia",
            Mf,
            Regular,
            [12, 12, 14, 12, 12, 15],
            holding(),
        ),
        card(
            9,
            "Dario Quintàs",
            "Real Olmedo",
            "Hispania",
            Mf,
            Legend,
            [16, 10, 18, 11, 17, 14],
            winger(),
        ),
        card(
            10,
            "Sven Kalvø",
            "Nordhavn",
            "Nordland",
            Fw,
            Rare,
            [18, 6, 14, 17, 15, 13],
            target_man(),
        ),
        card(
            11,
            "Marco Dell'Àrco",
            "Fiorentia",
            "Italia",
            Fw,
            Special,
            [19, 5, 16, 14, 18, 12],
            striker(),
        ),
        card(
            12,
            "Ryō Takamine",
            "Kitakaze",
            "日本",
            Gk,
            Regular,
            [2, 13, 8, 11, 9, 13],
            keeper(),
        ),
        card(
            13,
            "Bento Araújo",
            "Atlântico",
            "Lusitânia",
            Df,
            Regular,
            [8, 13, 10, 14, 12, 12],
            full_back(),
        ),
        card(
            14,
            "Ilya Varenko",
            "Dnipro Stal",
            "Rutenia",
            Mf,
            Regular,
            [11, 14, 12, 15, 11, 16],
            holding(),
        ),
        card(
            15,
            "Kenji Morioka",
            "Kitakaze",
            "日本",
            Fw,
            Regular,
            [15, 6, 12, 12, 17, 13],
            striker(),
        ),
        card(
            16,
            "Paolo Sàntoro",
            "Fiorentia",
            "Italia",
            Fw,
            Legend,
            [20, 7, 17, 16, 16, 14],
            striker(),
        ),
        card(
            17,
            "Aurelio Nuñez",
            "Real Olmedo",
            "Hispania",
            Df,
            Rare,
            [10, 17, 12, 14, 15, 14],
            Aptitude::default(),
        ),
        card(
            18,
            "Mathis Lefèvre",
            "Lyonnaise",
            "Gallia",
            Mf,
            Special,
            [15, 12, 16, 11, 14, 15],
            winger(),
        ),
        card(
            19,
            "Daichi Kurogane",
            "Kitakaze",
            "日本",
            Df,
            Regular,
            [7, 14, 10, 15, 12, 16],
            centre_back(),
        ),
        card(
            20,
            "Gianluca Pèllico",
            "Porto Azzurro",
            "Italia",
            Gk,
            AllTime,
            [4, 20, 11, 15, 10, 16],
            keeper(),
        ),
    ]
}

/// A 4-4-2 with one card out of position (a striker on the left of midfield); Nuñez on the bench has no position
/// data, like 2017-18's own cards; a full bench but one.
pub fn board() -> Board {
    let cards = cards();
    let mut slots = [None; 16];
    // keeper, back four, midfield four, two forwards
    for (slot, ci) in [0, 1, 2, 3, 4, 10, 6, 7, 8, 9, 15].into_iter().enumerate() {
        slots[slot] = Some(ci);
    }
    for (b, ci) in [11, 5, 13, 16].into_iter().enumerate() {
        slots[11 + b] = Some(ci);
    }
    Board {
        cards,
        slots,
        spots: [
            pos2(0.5, 0.92),
            pos2(0.14, 0.7),
            pos2(0.38, 0.74),
            pos2(0.62, 0.74),
            pos2(0.86, 0.7),
            pos2(0.14, 0.47),
            pos2(0.38, 0.52),
            pos2(0.62, 0.52),
            pos2(0.86, 0.47),
            pos2(0.33, 0.18),
            pos2(0.67, 0.18),
        ],
    }
}
