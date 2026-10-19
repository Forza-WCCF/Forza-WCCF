//! The stand-ins: the 2017–18 ones ported from `_aime_reader.py`, `_aimedb.py` and `_gamesvr.py` (step 10, piece J1)
//! the 2010–11 club card reader (piece D), network stand-ins (piece C), I/O board (piece E) and scene service (piece B).
//! Each service runs on std threads with blocking I/O ([`crate::service`]); dropping its [`Handle`] stops it. The
//! game server and the VS matcher the server shares live in [`crate::online`].

#[cfg(windows)]
pub mod aime;
pub mod aimedb;
#[cfg(windows)]
pub mod card_reader;
#[cfg(windows)]
pub mod io_board;
pub mod network;
#[cfg(windows)]
pub mod scenes;

pub use crate::service::{Handle, Log};
pub(crate) use crate::service::{accept_loop, read_some};
