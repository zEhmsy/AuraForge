//! Log sink: stderr for the console daemon, a file for the tray app.

use std::fmt::Arguments;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Sends every following line to `path`, truncating it first.
pub fn to_file(path: &Path) -> std::io::Result<()> {
    let file = File::create(path)?;
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    Ok(())
}

pub fn write(args: Arguments) {
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    match file.as_mut() {
        Some(f) => {
            let _ = writeln!(f, "[auraforge] {args}");
        }
        None => eprintln!("[auraforge] {args}"),
    }
}
