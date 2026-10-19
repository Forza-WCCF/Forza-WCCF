//! The club card steps of a 2010-11 start and stop (port of `legacy/2010-11/code/card_steps.py` and the `cards\` modules
//! it runs): a finished manager transfer put away and a CLUB CARD switch ([`wallet`]), the bad endings cleared
//! ([`edit`]), the cards on the table following the club ([`formation`]) and data\club_view.txt ([`view`]), all read
//! with [`decode`].

pub mod decode;
pub mod edit;
mod schema;
