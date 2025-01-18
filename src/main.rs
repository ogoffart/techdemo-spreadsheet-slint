#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), slint::PlatformError> {
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).

    spreadsheet_techdemo::run()
}

// When compiling to web using trunk:
#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_logger::init(wasm_logger::Config::new(log::Level::Debug));
    spreadsheet_techdemo::run().unwrap();
}
