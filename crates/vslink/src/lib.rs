//! Direct VS link (docs/ARCHITECTURE.md). `core` is the sans-IO routing logic, `net` the iroh endpoint, sockets and
//! gmsvr client around it; `arcade` the tunnel from a seat to a 2017-18 arcade on another PC (M2).

pub mod arcade;
pub mod core;
pub mod net;

pub use net::{Config, Link, spawn};
