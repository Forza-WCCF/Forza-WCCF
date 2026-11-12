//! Setup, undo and the game's text for Launcher.exe and the run, in this process (`forza::setup`, `forza::text`; before
//! 2026-10-20, setup.py and english.py run by the kit's Python).  Each is refused while a game program runs from the
//! game's folders, as the scripts refused; the lines go to the `Ui` one by one.  The exit codes are the scripts': 0 done,
//! 1 failed, 2 no game (not set up), and [`RUNNING`].  Their own lines are `run::Text`: the player's language in the window,
//! English in the logs, and so are forza's (`forza::Line`, [`tell_line`]).

use std::path::Path;

use forza::setup::{e1718, find_edition_game, kit_setup};
use forza::text::{self, Lang, Mode};

use crate::run::{Text, Ui, tell, tell_line};
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
    kit_setup(ed, kit.dir(), &folder, undo, false, &|lead, l| tell_line(ui, lead, l))
}

/// 2017-18, before a start's guard check: why its runtime needs setup again. An older Forza-WCCF set it up (an update,
/// new files unzipped over it; `e1718::stale`), or else `check`, the guard check, refuses it (no or an unreadable
/// setup-hashes.json, a guarded file an update or an antivirus put back). None: it passes as it is.
pub(crate) fn why_again(kit: &Kit, game: &Path, check: impl Fn() -> Result<(), String>) -> Option<Text> {
    match e1718::stale(kit.dir(), game) {
        Some(why) => Some(Text::key("launcher.run_setup_again").arg("why", why)),
        None => check()
            .err()
            .map(|why| Text::key("launcher.run_setup_guard").arg("why", why)),
    }
}

/// 2017-18 inside a start: the runtime set up again once with `set_up` (`run::Params::setup_1718`), `why` said first
/// and its result after. False: it could not be, and nothing may start.
pub(crate) fn again(
    kit: &Kit,
    game: &Path,
    why: &Text,
    set_up: fn(&Path, &Path) -> Result<(), String>,
    ui: &dyn Ui,
) -> bool {
    tell(ui, "", why);
    if running(game, "launcher.run_running_setup", ui) {
        return false;
    }
    match set_up(kit.dir(), game) {
        Ok(()) => {
            tell(ui, "  ok    ", &Text::key("launcher.run_setup_again_done"));
            true
        }
        Err(e) => {
            tell(
                ui,
                "  FAIL  ",
                &Text::key("launcher.run_setup_again_failed").arg("why", e),
            );
            false
        }
    }
}

/// What a start's [`again`] runs for the game: setup's game part, as SET UP runs it (`e1718::setup_game`), and the card
/// catalogue a play folder needs (made when it is missing; SET UP makes it too).
pub(crate) fn setup_1718(kit: &Path, game: &Path) -> Result<(), String> {
    e1718::setup_game(kit, game, &e1718::Spec::game())
        .map(drop)
        .map_err(|e| e.to_string())?;
    catalogue_1718(kit, game)
}

/// The card catalogue of the game (`overlay\catalogue.tsv`, `e1718::setup_catalogue`): made from the game when the
/// play folder has none, kept as it is when it has one (an older kit set the folder up before the catalogue existed).
fn catalogue_1718(kit: &Path, game: &Path) -> Result<(), String> {
    e1718::setup_catalogue(kit, game, false, &|_: &str, _: &forza::Line| {}).map_err(|e| e.to_string())
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
        &|lead, l| tell_line(ui, lead, l),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch;

    /// A play folder set up by an older kit has no catalogue.tsv, and the board then shows no cards: the automatic
    /// setup before a start builds it, and leaves the one already there as it is.
    #[test]
    fn a_missing_catalogue_is_built_and_a_present_one_kept() {
        let kit = scratch();
        let game = kit.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        let overlay = kit.path().join(crate::Edition::E1718.layout().overlay);
        std::fs::create_dir_all(&overlay).unwrap();
        // no catalogue, and no game files to make one from: the build is tried, and its failure is setup's
        let err = catalogue_1718(kit.path(), &game).unwrap_err();
        assert!(err.contains("catalogue"), "{err}");
        let cat = overlay.join("catalogue.tsv");
        std::fs::write(&cat, "card_no\tname\n").unwrap();
        catalogue_1718(kit.path(), &game).unwrap();
        assert_eq!(
            std::fs::read_to_string(&cat).unwrap(),
            "card_no\tname\n",
            "one there is not built again"
        );
    }
}
