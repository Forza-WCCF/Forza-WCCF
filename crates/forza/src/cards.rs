//! The player's cards, read at runtime from the player's own folders: the catalogue setup made from their game
//! (`catalogue.tsv`, read by its header's names, so both editions' column sets work) and the game's card table
//! (`fpr_table0.txt`, what FPR_Emu.exe hands the game as table 0) with the formation copy the old Python panel keeps
//! beside it (`fpr_panel_state.json`). Ported from the C panel (`native/2010-11/panel/wccfpanel.c`: `parse_table`,
//! `write_table_locked`, `write_panel_state_locked`, `board_add_locked`, `load_catalogue`); both files come out byte
//! for byte as it writes them.

pub mod catalogue;
mod catalogue_tsv;
pub mod codes;
mod deal;
pub mod formations;
pub mod hq;
pub mod lineup;
pub mod links;
pub mod pictures;
mod placement;
pub mod steps;
mod table;

pub use catalogue_tsv::load_catalogue;
pub use deal::{deal, deal_class, deal_now, deal_tier};
pub(crate) use placement::round3;
#[cfg(test)]
pub(crate) use placement::unknown;
pub use placement::{AddRefused, add, board, table_after};
pub(crate) use table::PITCH;
pub use table::{
    BENCH_X, BENCH_Y, MAX_CARDS, Placed, TableFile, bench_band, mirror_of, parse_table, state_text,
    swap_in, table_text,
};
