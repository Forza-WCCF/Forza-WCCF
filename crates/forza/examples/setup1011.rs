//! `setup1011 [undo] <kit folder> <game folder> [cards]`: the 2010-11 install steps (`forza::setup::e1011`), for
//! comparing with `code\setup.py`.  Run it on a copy of the game, never on the original.

use forza::setup::e1011;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let undo = args.first().is_some_and(|a| a == "undo");
    let cards = args.last().is_some_and(|a| a == "cards");
    args.retain(|a| a != "undo" && a != "cards");
    let [kit, game] = &args[..] else {
        eprintln!("usage: setup1011 [undo] <kit folder> <game folder> [cards]");
        std::process::exit(2);
    };
    let Some(game) = e1011::find_game(game) else {
        println!(
            "no WCCF 2010-11 game files in {game} (client_Release.exe, control_Release.exe, match_Release.exe)"
        );
        std::process::exit(2);
    };
    let kit = std::path::Path::new(kit);
    let mode = if undo { "undo" } else { "setup" };
    println!("WCCF 2010-11 kit - {mode} - game: {}", game.display());
    let say = |s: &str| println!("{s}");
    let done = if undo {
        e1011::undo(kit, &game, &say)
    } else {
        e1011::run(kit, &game, cards, &say)
    };
    if let Err(e) = done {
        println!("  FAIL  {e}");
        std::process::exit(1);
    }
    if !undo {
        println!("Setup done. Start the game with Launcher.exe.");
    }
}
