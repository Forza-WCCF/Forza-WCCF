//! `cargo run -p forza --example catalogue -- 2010-11|2017-18 GAME [OUT [--catalogue-only]]`: the catalogue and card
//! pictures from a game folder (the comparison with the Python makers). Without OUT it only reads and checks.
use forza::Edition;
use forza::cards::catalogue::Output;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let only = a.iter().any(|x| x == "--catalogue-only");
    let a: Vec<&String> = a.iter().filter(|x| *x != "--catalogue-only").collect();
    let (ed, game, out) = match a.as_slice() {
        [ed, game] => (ed, game, ""),
        [ed, game, out] => (ed, game, out.as_str()),
        _ => {
            eprintln!("usage: catalogue 2010-11|2017-18 GAME [OUT [--catalogue-only]]");
            std::process::exit(2);
        }
    };
    let Some(edition) = [Edition::E1011, Edition::E1718]
        .into_iter()
        .find(|e| e.name() == ed.as_str())
    else {
        eprintln!("edition must be 2010-11 or 2017-18");
        std::process::exit(2);
    };
    let output = match (out.is_empty(), only) {
        (true, _) => Output::CheckOnly,
        (false, true) => Output::CatalogueOnly,
        (false, false) => Output::All,
    };
    std::process::exit(forza::cards::catalogue::run(
        edition,
        game.as_ref(),
        out.as_ref(),
        output,
        &|lead, l| println!("{lead}{l:?}"),
    ));
}
