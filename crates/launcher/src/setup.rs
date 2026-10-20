//! Setup, undo and the game's text for Launcher.exe and the run, in this process (`forza::setup`, `forza::text`; before
//! 2026-10-20, setup.py and english.py run by the kit's Python).  Each is refused while a game program runs from the
//! game's folders, as the scripts refused; the lines go to `say` one by one.  The exit codes are the scripts': 0 done,
//! 1 failed, 2 no game (not set up), and [`RUNNING`].

use std::path::Path;

use forza::setup::{find_edition_game, kit_setup};
use forza::text::{self, Lang, Mode};

use crate::{Kit, choices, procs};

/// Refused: a game program runs from the game's folders.
pub const RUNNING: i32 = 3;

fn running(game: &Path, what: &str, say: &dyn Fn(&str)) -> bool {
    let on = !procs::game_processes(game).is_empty();
    if on {
        say(&format!(
            "  FAIL  the game is running - close its window first, then {what}"
        ));
    }
    on
}

/// Set up (`undo` false) or undo the game in `folder` (an empty one: the game data\settings.json records).
pub fn game_folder(kit: &Kit, folder: &str, undo: bool, say: &dyn Fn(&str)) -> i32 {
    let ed = kit.edition();
    let folder = if folder.trim().is_empty() {
        choices::setting(&kit.settings_file(), "game").unwrap_or_default()
    } else {
        folder.to_owned()
    };
    if let Some(game) = find_edition_game(ed, &folder)
        && running(&game, if undo { "undo" } else { "run setup again" }, say)
    {
        return RUNNING;
    }
    kit_setup(ed, kit.dir(), &folder, undo, false, say)
}

/// The game's text into `lang`: "en" English, "it" Italian, "" the original Japanese.
pub fn game_text(kit: &Kit, lang: &str, say: &dyn Fn(&str)) -> i32 {
    let ed = kit.edition();
    let recorded = choices::setting(&kit.settings_file(), "game").unwrap_or_default();
    let Some(game) = find_edition_game(ed, &recorded) else {
        say("The game is not set up yet - set it up first: Launcher.exe, SETUP.");
        return 2;
    };
    if running(&game, "switch the text again", say) {
        return RUNNING;
    }
    let (mode, lang) = match lang {
        "en" => (Mode::On, Lang::En),
        "it" => (Mode::On, Lang::It),
        _ => (Mode::Off, Lang::En),
    };
    text::run(
        ed,
        &game,
        &kit.data().join("english_backup"),
        kit.dir(),
        mode,
        lang,
        say,
    )
}
