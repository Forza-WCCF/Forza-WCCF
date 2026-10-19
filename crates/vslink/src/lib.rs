//! Direct VS link (docs/IROH_VS_PLAN.md). `core` is the sans-IO routing logic, `net` the iroh endpoint, sockets and
//! gmsvr client around it.

pub mod core;
pub mod net;

pub use net::{Config, Link, spawn};
