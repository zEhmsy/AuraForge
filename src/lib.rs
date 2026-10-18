//! Ambilight and League of Legends lighting for the Drevo Tyrfing V2 keyboard
//! and ASUS Aura ARGB fans. Shared by the console daemon and the tray app.

/// Writes a line to the log sink: stderr, or the file chosen with `logging::to_file`.
macro_rules! log {
    ($($arg:tt)*) => { $crate::logging::write(format_args!($($arg)*)) };
}

pub mod aura;
pub mod capture;
pub mod color;
pub mod config;
pub mod effects;
pub mod engine;
pub mod fanout;
pub mod govee;
pub mod hid;
pub mod layout;
pub mod lights;
pub mod logging;
pub mod lol;
pub mod protocol;
