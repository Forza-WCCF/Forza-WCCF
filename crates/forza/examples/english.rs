//! `cargo run -p forza --example english -- 2010-11|2017-18 on|off|check [it] GAME BACKUP SHEETS`: the game text in
//! English or Italian (the comparison with `english.py`; the shipped path still runs Python). SHEETS is
//! `editions/<edition>`; BACKUP is the kit's `data/english_backup`.
use forza::{
    Edition,
    text::{Lang, Mode, run},
};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (ed, mode, lang, paths) = match a.as_slice() {
        [e, m, l, paths @ ..] if l == "it" => (e, m, Lang::It, paths),
        [e, m, paths @ ..] => (e, m, Lang::En, paths),
        _ => (&a[0], &a[0], Lang::En, &a[..0]),
    };
    let edition = [Edition::E1011, Edition::E1718]
        .into_iter()
        .find(|e| e.name() == ed);
    let mode = [("on", Mode::On), ("off", Mode::Off), ("check", Mode::Check)]
        .into_iter()
        .find(|(n, _)| n == mode);
    let (Some(edition), Some((_, mode)), [game, backup, sheets]) = (edition, mode, paths) else {
        eprintln!("usage: english 2010-11|2017-18 on|off|check [it] GAME BACKUP SHEETS");
        std::process::exit(2);
    };
    std::process::exit(run(
        edition,
        game.as_ref(),
        backup.as_ref(),
        sheets.as_ref(),
        mode,
        lang,
        &|s| println!("{s}"),
    ));
}
