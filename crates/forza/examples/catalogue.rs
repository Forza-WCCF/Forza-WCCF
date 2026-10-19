//! `cargo run -p forza --example catalogue -- 2010-11|2017-18 GAME OUT`: the catalogue and card pictures from a game
//! folder (the comparison with the Python makers; the shipped path still runs Python).
use forza::Edition;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [ed, game, out] = a.as_slice() else {
        eprintln!("usage: catalogue 2010-11|2017-18 GAME OUT");
        std::process::exit(2);
    };
    let Some(edition) = [Edition::E1011, Edition::E1718]
        .into_iter()
        .find(|e| e.name() == ed)
    else {
        eprintln!("edition must be 2010-11 or 2017-18");
        std::process::exit(2);
    };
    std::process::exit(forza::cards::catalogue::run(
        edition,
        game.as_ref(),
        out.as_ref(),
        &|s| println!("{s}"),
    ));
}
