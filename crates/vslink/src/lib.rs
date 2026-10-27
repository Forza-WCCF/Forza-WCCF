//! Direct VS link (docs/IROH_VS_PLAN.md). `core` is the sans-IO routing logic, `net` the iroh endpoint, sockets and
//! gmsvr client around it; `shop` the tunnel from a seat to a 2017-18 shop on another PC (M2).

pub mod core;
pub mod net;
pub mod shop;

pub use net::{Config, Link, spawn};
