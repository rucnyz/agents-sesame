pub mod adapter;
pub mod adapters;
pub mod config;
pub mod index;
pub mod query;
pub mod search;
pub mod session;

mod cli;
pub(crate) mod tui;
mod update;

pub use cli::run;
