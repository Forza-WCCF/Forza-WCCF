use crate::board::{Aptitude, Card, Position, Rarity};
use std::io;
use std::path::Path;

// ---- the catalogue ----

/// The white ATLE-RE reprints the 2017-18 game treats as legends beside every ALL TIME LEGEND card (its own list of
/// card numbers, read from the client).
const LEGEND_REPRINTS: [u32; 18] = [
    4659, 5747, 5748, 5749, 5752, 5754, 5755, 6151, 6153, 6154, 6299, 6417, 6635, 6720, 6721, 7131,
    7132, 7133,
];

/// `catalogue.tsv` (one card per line, tab-separated, UTF-8, a header naming the columns) -> its cards, in the
/// file's order. Columns by name: card_no, season, rarity_name, is_placeholder (1 = skipped), name_full_latin,
/// name_short_latin, position_name, club_name, nationality_name, stat_offence .. stat_stamina, stats_total, and the
/// record_index (the card's player_data.bin record, for [`links`]), card_code (2017-18's printed code), the
/// 16 role ratings as the first 16 numbers of hidden_params (2010-11) or hidden_4df_523 (2017-18), kept only when
/// all are 1-10; else the 12 position flags of tail_5a4_61f (hex) rate their roles 8.
pub fn load_catalogue(path: &Path) -> io::Result<Vec<Card>> {
    let mut r = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .quoting(false)
        .flexible(true)
        .from_path(path)
        .map_err(io::Error::other)?;
    let head = r.byte_headers().map_err(io::Error::other)?.clone();
    let col = |names: &[&str]| {
        head.iter()
            .position(|h| names.iter().any(|n| h == n.as_bytes()))
    };
    let c_no = col(&["card_no"]).ok_or_else(|| io::Error::other("no card_no column"))?;
    let c_rec = col(&["record_index"]);
    let c_code = col(&["card_code"]);
    let c_pic = col(&["image_archive_path"]);
    let [
        season,
        born,
        rarity,
        placeholder,
        full,
        short,
        pos,
        club,
        nat,
        total,
        roles,
        tail,
    ] = [
        &["season"][..],
        &["birth_date"],
        &["rarity_name"],
        &["is_placeholder"],
        &["name_full_latin"],
        &["name_short_latin"],
        &["position_name"],
        &["club_name"],
        &["nationality_name"],
        &["stats_total"],
        &["hidden_params", "hidden_4df_523"],
        &["tail_5a4_61f"],
    ]
    .map(col);
    let stats = [
        "stat_offence",
        "stat_defence",
        "stat_technique",
        "stat_power",
        "stat_speed",
        "stat_stamina",
    ]
    .map(|n| col(&[n]));
    let mut out = Vec::new();
    for rec in r.byte_records() {
        let Ok(rec) = rec else { continue };
        let get = |c: Option<usize>| {
            c.and_then(|c| rec.get(c))
                .map(|b| String::from_utf8_lossy(b).trim().to_owned())
                .unwrap_or_default()
        };
        let num = |c: Option<usize>| get(c).parse::<u8>().unwrap_or(0);
        let Ok(number) = get(Some(c_no)).parse::<u32>() else {
            continue;
        };
        if number == 0 || get(placeholder) == "1" {
            continue;
        }
        let ratings: Vec<u8> = get(roles)
            .split_whitespace()
            .take(16)
            .map(|v| v.parse().unwrap_or(0))
            .collect();
        let rarity = match get(rarity).as_str() {
            "ALL TIME LEGEND" => Rarity::AllTime,
            "LEGEND" => Rarity::Legend,
            "RARE" => Rarity::Rare,
            "SPECIAL" => Rarity::Special,
            _ => Rarity::Regular,
        };
        let mut aptitude = Aptitude::default();
        if ratings.len() == 16 && ratings.iter().all(|r| (1..=10).contains(r)) {
            aptitude.roles.copy_from_slice(&ratings);
        } else if let Some(flags) = get(tail).get(10..34) {
            // 2017-18 cards of editions 36+ have no ratings, only the 12 position flags (tail bytes 5..17)
            aptitude = Aptitude::from_flags(flags);
        }
        out.push(Card {
            number,
            record: get(c_rec).parse().ok(),
            name: get(full),
            short: get(short),
            club: get(club),
            country: get(nat),
            season: get(season),
            born: get(born),
            pos: match get(pos).as_str() {
                "GK" => Position::Gk,
                "DF" => Position::Df,
                "FW" => Position::Fw,
                _ => Position::Mf,
            },
            rarity,
            code: Some(get(c_code))
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| mvp_mark(&get(c_pic)).into()),
            // 2017-18 only (the catalogue with card codes): 2010-11's legends print their stats
            secret: c_code.is_some()
                && (rarity == Rarity::AllTime || LEGEND_REPRINTS.contains(&number)),
            stats: stats.map(num),
            total: num(total),
            aptitude,
        });
    }
    Ok(out)
}

/// An older MVP card's mark, which the catalogue has no card code for: its picture's name carries it
/// (`EMVP0506_RONALDINHO_f.dds` -> EMVP, `WMVP_RONALDINHO.dds` -> WMVP, `CMVP_001_NEDVED.dds` -> CMVP, `MVP_1.dds` -> MVP).
fn mvp_mark(picture: &str) -> &str {
    let file = picture.rsplit('/').next().unwrap_or_default();
    file.split(['_', '.'])
        .map(|t| t.trim_end_matches(|c: char| c.is_ascii_digit()))
        .find(|t| ["MVP", "EMVP", "WMVP", "CMVP"].contains(t))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn older_mvp_cards_get_their_mark_from_the_picture() {
        for (pic, want) in [
            ("data/player_card/EMVP0506_RONALDINHO_f.dds", "EMVP"),
            ("data/player_card/WMVP0506_RONALDINHO_f.dds", "WMVP"),
            ("data/player_card/WMVP_RONALDINHO.dds", "WMVP"),
            ("data/player_card/CMVP_001_NEDVED.dds", "CMVP"),
            ("data/player_card/MVP_1.dds", "MVP"),
            ("data/player_card/CNS0405_192_RONALDINHO.dds", ""),
            ("data/player_card/1718_CardImage_8993.dds", ""),
            ("", ""),
        ] {
            assert_eq!(mvp_mark(pic), want, "{pic}");
        }
    }

    /// The catalogue by its header's names, in either edition's column set; placeholders skipped; the roles kept only
    /// when all 16 are 1-10.
    #[test]
    fn the_catalogue_is_read_by_column_names() {
        let d = tempfile::tempdir().unwrap();
        let roles: String = (0..16)
            .map(|i| (1 + i % 10).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let path = d.path().join("catalogue.tsv");
        let head = "card_no\trecord_index\tseason\trarity_name\tis_placeholder\tname_full_latin\tname_short_latin\t\
                    position_name\tclub_name\tnationality_name\tstat_offence\tstat_defence\tstat_technique\tstat_power\t\
                    stat_speed\tstat_stamina\tstats_total\thidden_4df_523\tcard_code\ttail_5a4_61f\n";
        let rows = format!(
            "12\t0\t10-11\tLEGEND\t0\tAldo Bàrtoli\tBÀRTOLI\tFW\tPorto Azzurro\tItalia\t18\t5\t16\t14\t17\t12\t82\t{roles} 99 3\t\t\n\
             13\t1\t10-11\tREGULAR\t1\tNobody\tNOBODY\tMF\t\t\t1\t1\t1\t1\t1\t1\t60\t\n\
             14\t2\t15-16\tALL TIME LEGEND\t0\tKenji Sato\tSATO\tGK\tKitakaze\t日本\t3\t15\t9\t12\t8\t10\t71\t0 0 0\tHOLE\t\n\
             15\t3\t17-18\tREGULAR\t0\tNew Card\tNEW\tMF\tX\tY\t1\t1\t1\t1\t1\t1\t60\t0 0 0\t\t0000000000010100000000000000000000000000"
        );
        fs::write(&path, format!("{head}{rows}")).unwrap();
        let cat = load_catalogue(&path).unwrap();
        assert_eq!(cat.len(), 3);
        assert_eq!((cat[0].record, cat[1].record), (Some(0), Some(2)));
        let a = &cat[0];
        assert_eq!(
            (a.number, a.name.as_str(), a.short.as_str(), a.pos, a.rarity),
            (12, "Aldo Bàrtoli", "BÀRTOLI", Position::Fw, Rarity::Legend)
        );
        assert_eq!((a.stats, a.total), ([18, 5, 16, 14, 17, 12], 82));
        assert_eq!(a.aptitude.roles[..3], [1, 2, 3]);
        assert_eq!(a.aptitude.roles[15], 6);
        // no ratings, flags CF and SS (hex bytes 5 and 6 of the tail): both roles rated 8
        let c = &cat[2];
        assert_eq!((c.aptitude.roles[0], c.aptitude.roles[3]), (8, 8));
        assert_eq!(c.aptitude.roles.iter().filter(|&&r| r > 0).count(), 2);
        let b = &cat[1];
        assert_eq!(
            (b.country.as_str(), b.rarity, b.pos),
            ("日本", Rarity::AllTime, Position::Gk)
        );
        assert!(!b.aptitude.known());
        assert_eq!(
            (a.code.as_str(), b.code.as_str(), b.series()),
            ("", "HOLE", "HOLE")
        );
        // the ALL TIME LEGEND keeps its stats secret, the LEGEND does not
        assert_eq!((a.secret, b.secret), (false, true));
        let series = [
            "WGK-3",
            "MVP-1",
            "KLE",
            "12",
            "ATLE-RE-4",
            "P WOS-2",
            "JT-A11",
            "",
        ]
        .map(crate::board::series);
        assert_eq!(
            series,
            ["WGK", "MVP", "KLE", "", "ATLE-RE", "P WOS", "JT-A", ""]
        );
    }
}
