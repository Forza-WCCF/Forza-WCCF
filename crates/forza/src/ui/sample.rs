//! Invented sample data for the gallery and the snapshots: players, clubs and countries that do not exist, with
//! Italian accents and Japanese names so every screen is tried with them.

use super::board::{Board, Card, Position, Rarity};
use egui::pos2;

fn card(
    number: u16,
    name: &str,
    club: &str,
    country: &str,
    pos: Position,
    rarity: Rarity,
    stats: [u8; 6],
) -> Card {
    Card {
        number,
        name: name.into(),
        club: club.into(),
        country: country.into(),
        season: "10-11".into(),
        pos,
        rarity,
        stats,
    }
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
        ),
        card(
            2,
            "Niccolò Bàrtoli",
            "Porto Azzurro",
            "Italia",
            Df,
            Legend,
            [9, 19, 14, 15, 13, 16],
        ),
        card(
            3,
            "Kōji Asanuma",
            "Kitakaze",
            "日本",
            Df,
            Regular,
            [7, 15, 11, 13, 14, 15],
        ),
        card(
            4,
            "Hélder Maçãs",
            "Atlântico",
            "Lusitânia",
            Df,
            Special,
            [11, 16, 13, 12, 16, 14],
        ),
        card(
            5,
            "Jens Ødegård",
            "Nordhavn",
            "Nordland",
            Df,
            Regular,
            [6, 14, 9, 16, 11, 13],
        ),
        card(
            6,
            "Lucà Ferrè",
            "Porto Azzurro",
            "Italia",
            Mf,
            AllTime,
            [17, 13, 20, 13, 15, 17],
        ),
        card(
            7,
            "Taiga Hoshino",
            "Kitakaze",
            "日本",
            Mf,
            Rare,
            [14, 11, 17, 10, 16, 18],
        ),
        card(
            8,
            "Émile Fourcade",
            "Lyonnaise",
            "Gallia",
            Mf,
            Regular,
            [12, 12, 14, 12, 12, 15],
        ),
        card(
            9,
            "Dario Quintàs",
            "Real Olmedo",
            "Hispania",
            Mf,
            Legend,
            [16, 10, 18, 11, 17, 14],
        ),
        card(
            10,
            "Sven Kalvø",
            "Nordhavn",
            "Nordland",
            Fw,
            Rare,
            [18, 6, 14, 17, 15, 13],
        ),
        card(
            11,
            "Marco Dell'Àrco",
            "Fiorentia",
            "Italia",
            Fw,
            Special,
            [19, 5, 16, 14, 18, 12],
        ),
        card(
            12,
            "Ryō Takamine",
            "Kitakaze",
            "日本",
            Gk,
            Regular,
            [2, 13, 8, 11, 9, 13],
        ),
        card(
            13,
            "Bento Araújo",
            "Atlântico",
            "Lusitânia",
            Df,
            Regular,
            [8, 13, 10, 14, 12, 12],
        ),
        card(
            14,
            "Ilya Varenko",
            "Dnipro Stal",
            "Rutenia",
            Mf,
            Regular,
            [11, 14, 12, 15, 11, 16],
        ),
        card(
            15,
            "Kenji Morioka",
            "Kitakaze",
            "日本",
            Fw,
            Regular,
            [15, 6, 12, 12, 17, 13],
        ),
        card(
            16,
            "Paolo Sàntoro",
            "Fiorentia",
            "Italia",
            Fw,
            Legend,
            [20, 7, 17, 16, 16, 14],
        ),
        card(
            17,
            "Aurelio Nuñez",
            "Real Olmedo",
            "Hispania",
            Df,
            Rare,
            [10, 17, 12, 14, 15, 14],
        ),
        card(
            18,
            "Mathis Lefèvre",
            "Lyonnaise",
            "Gallia",
            Mf,
            Special,
            [15, 12, 16, 11, 14, 15],
        ),
        card(
            19,
            "Daichi Kurogane",
            "Kitakaze",
            "日本",
            Df,
            Regular,
            [7, 14, 10, 15, 12, 16],
        ),
        card(
            20,
            "Gianluca Pèllico",
            "Porto Azzurro",
            "Italia",
            Gk,
            AllTime,
            [4, 20, 11, 15, 10, 16],
        ),
    ]
}

/// A 4-4-2 with one card out of position (a forward on the left of midfield), a full bench but one.
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
            pos2(0.14, 0.73),
            pos2(0.38, 0.77),
            pos2(0.62, 0.77),
            pos2(0.86, 0.73),
            pos2(0.14, 0.47),
            pos2(0.38, 0.52),
            pos2(0.62, 0.52),
            pos2(0.86, 0.47),
            pos2(0.33, 0.18),
            pos2(0.67, 0.18),
        ],
    }
}
