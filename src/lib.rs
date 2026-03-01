pub mod adapter;
pub mod adapters;
pub mod config;
pub mod index;
pub mod query;
pub mod search;
pub mod session;

mod cli;
mod tui;

pub use cli::run;
