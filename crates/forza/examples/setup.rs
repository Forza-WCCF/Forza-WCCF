//! `setup [undo] <kit folder> <game folder>`: the 2017-18 install steps (`forza::setup`), for comparing with
//! `scripts\setup.py`.  Run it on a copy of the game, never on the original.

use forza::setup::e1718;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let undo = args.first().is_some_and(|a| a == "undo");
    if undo {
        args.remove(0);
    }
    let [kit, game] = &args[..] else {
        eprintln!("usage: setup [undo] <kit folder> <game folder>");
        std::process::exit(2);
    };
    let Some(game) = e1718::find_game(game) else {
        println!("no WCCF 2017-18 game files in {game} (client_Release.exe, control_Release.exe)");
        std::process::exit(2);
    };
    let (kit, spec) = (std::path::Path::new(kit), e1718::Spec::game());
    println!(
        "Forza-WCCF 2017-18 - {} - game: {}",
        if undo { "undo" } else { "setup" },
        game.display()
    );
    let done = if undo {
        e1718::undo(kit, &game, &spec)
    } else {
        e1718::run(kit, &game, &spec).map(|r| {
            for (k, _) in &r.0 {
                println!("  ok    {k}");
            }
        })
    };
    if let Err(e) = done {
        println!("  FAIL  {e}");
        std::process::exit(1);
    }
}
