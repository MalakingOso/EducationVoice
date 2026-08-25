//! The GUI's non-UI core: where the Python lives, what it says, and how to
//! run it.
//!
//! This crate keeps a library alongside its binaries, which Beamer does not.
//! Beamer's second binary `#[path]`-includes the modules it needs, and its
//! Cargo.toml has to set `test = false` on that target or every colocated
//! test compiles and runs a second time, overstating the suite. A library
//! both binaries link removes that whole problem.

pub mod config;
pub mod library;
pub mod paths;
pub mod proto;
pub mod roster;
pub mod runner;
pub mod ui;
