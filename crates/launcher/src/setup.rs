//! Setup, undo and the game's text for Launcher.exe and the run, in this process (`forza::setup`, `forza::text`; before
//! 2026-10-20, setup.py and english.py run by the kit's Python).  Each is refused while a game program runs from the
//! game's folders, as the scripts refused; the lines go to the `Ui` one by one.  The exit codes are the scripts': 0 done,
//! 1 failed, 2 no game (not set up), and [`RUNNING`].  Their own lines are `run::Text`: the player's language in the window,
//! English in the logs.

use std::path::Path;

use forza::setup::{find_edition_game, kit_setup};
use forza::text::{self, Lang, Mode};

use crate::run::{Text, Ui, tell};
use crate::{Kit, choices, procs, run};

/// Refused: a game program runs from the game's folders.
pub const RUNNING: i32 = 3;

/// The refusal's line for a step: `launcher.run_running_undo`, `_setup` or `_text`.
fn running(game: &Path, what: &'static str, ui: &dyn Ui) -> bool {
    let on = !procs::game_processes(game).is_empty();
    if on {
        tell(ui, "  FAIL  ", &Text::key(what));
    }
    on
}

fn undo_or_setup(undo: bool) -> &'static str {
    if undo {
        "launcher.run_running_undo"
    } else {
        "launcher.run_running_setup"
    }
}

/// The kit's admission for a step from Launcher (`run::Admission`), held for the whole step: refused while a run of this
/// kit starts or is on, in any Launcher window.
fn admitted(kit: &Kit, what: &'static str, ui: &dyn Ui) -> Option<run::Admission> {
    let a = run::admit(kit).filter(|_| !run::any_on(kit));
    if a.is_none() {
        tell(ui, "  FAIL  ", &Text::key(what));
    }
    a
}

/// Set up (`undo` false) or undo the game in `folder` (an empty one: the game data\settings.json records).
pub fn game_folder(kit: &Kit, folder: &str, undo: bool, ui: &dyn Ui) -> i32 {
    let Some(_admitted) = admitted(kit, undo_or_setup(undo), ui) else {
        return RUNNING;
    };
    folder_step(kit, folder, undo, ui)
}

/// [`game_folder`] inside a start, which holds the admission and is on.
pub(crate) fn folder_step(kit: &Kit, folder: &str, undo: bool, ui: &dyn Ui) -> i32 {
    let ed = kit.edition();
    let folder = if folder.trim().is_empty() {
        choices::setting(&kit.settings_file(), "game").unwrap_or_default()
    } else {
        folder.to_owned()
    };
    if let Some(game) = find_edition_game(ed, &folder)
        && running(&game, undo_or_setup(undo), ui)
    {
        return RUNNING;
    }
    kit_setup(ed, kit.dir(), &folder, undo, false, &|l| ui.say(l))
}

/// The game's text into `lang`: "en" English, "it" Italian, "" the original Japanese.
pub fn game_text(kit: &Kit, lang: &str, ui: &dyn Ui) -> i32 {
    let Some(_admitted) = admitted(kit, "launcher.run_running_text", ui) else {
        return RUNNING;
    };
    text_step(kit, lang, ui)
}

/// [`game_text`] inside a start, which holds the admission and is on.
pub(crate) fn text_step(kit: &Kit, lang: &str, ui: &dyn Ui) -> i32 {
    let ed = kit.edition();
    let recorded = choices::setting(&kit.settings_file(), "game").unwrap_or_default();
    let Some(game) = find_edition_game(ed, &recorded) else {
        tell(ui, "", &Text::key("launcher.run_not_set_up"));
        return 2;
    };
    if running(&game, "launcher.run_running_text", ui) {
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
        &|l| ui.say(l),
    )
}
