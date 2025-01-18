#![warn(clippy::all, rust_2018_idioms)]
mod app;
mod cell_cache;
mod debouncer;
mod http;

pub use app::run;

pub const API_HOST: Option<&'static str> = option_env!("API_HOST");
