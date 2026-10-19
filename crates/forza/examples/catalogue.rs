//! `cargo run -p forza --example catalogue -- GAME OUT`: the catalogue and card pictures from a game folder (the
//! comparison with the Python maker; the shipped path still runs Python).
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [game, out] = a.as_slice() else {
        eprintln!("usage: catalogue GAME OUT");
        std::process::exit(2);
    };
    std::process::exit(forza::cards::catalogue::run(
        game.as_ref(),
        out.as_ref(),
        &|s| println!("{s}"),
    ));
}
