//! Where each edition's kit keeps its parts.  The one table the launcher (`Kit`), `forza::setup` and `cargo xtask check`
//! read, so a folder is renamed in one place.

use crate::Edition;

/// Folder names inside a kit folder, per edition (see [`Edition::layout`]).
pub struct Layout {
    /// The Python scripts (setup.py, english.py) until step 12 drops them.
    pub scripts: &'static str,
    /// The kit's compiled programs: the hook, the stand-ins, the card-table helper.
    pub bin: &'static str,
    /// Where the programs named in [`Layout::overlay_files`] live (the run's overlay), `bin` when none do.
    pub overlay: &'static str,
    pub overlay_files: &'static [&'static str],
    /// Made by setup from the player's game: catalogue.tsv, cards\ (a path below the kit folder).
    pub cache: &'static str,
}

impl Layout {
    /// The folder (below the kit folder) that holds the program `name`.
    pub fn program_dir(&self, name: &str) -> &'static str {
        if self.overlay_files.contains(&name) {
            self.overlay
        } else {
            self.bin
        }
    }
}

pub const LAYOUT_1011: Layout = Layout {
    scripts: "code",
    bin: "programs",
    overlay: "programs",
    overlay_files: &[],
    cache: "data/cache",
};

pub const LAYOUT_1718: Layout = Layout {
    scripts: "scripts",
    bin: "bin",
    overlay: "overlay",
    overlay_files: &["inject.exe", "forza_panel.dll", "netguard.dll"],
    cache: "overlay",
};

impl Edition {
    pub fn layout(self) -> &'static Layout {
        match self {
            Self::E1011 => &LAYOUT_1011,
            Self::E1718 => &LAYOUT_1718,
        }
    }
}
