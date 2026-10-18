//! Physical key positions of the Tyrfing V2 ISO UK tenkeyless board.
//!
//! Units are key widths (1u ≈ 19 mm); rows sit at 0, 1.25, 2.25, ... to
//! account for the gap under the function row. Codes are USB HID usage IDs,
//! which is what `KM_IO_SET_KEY_COLOR` expects (`UHSC_*` in `wkb/kb_def.py`).
//! Fn has no usage ID and is left untouched.

pub const WIDTH: f32 = 18.25;
pub const HEIGHT: f32 = 6.25;

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub code: u8,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const fn key(code: u8, x: f32, y: f32, w: f32) -> Key {
    Key { code, x, y, w, h: 1.0 }
}

const R0: f32 = 0.0;
const R1: f32 = 1.25;
const R2: f32 = 2.25;
const R3: f32 = 3.25;
const R4: f32 = 4.25;
const R5: f32 = 5.25;

pub const ISO_UK: [Key; 87] = [
    // Function row.
    key(0x29, 0.0, R0, 1.0), // Esc
    key(0x3A, 2.0, R0, 1.0),
    key(0x3B, 3.0, R0, 1.0),
    key(0x3C, 4.0, R0, 1.0),
    key(0x3D, 5.0, R0, 1.0),
    key(0x3E, 6.5, R0, 1.0),
    key(0x3F, 7.5, R0, 1.0),
    key(0x40, 8.5, R0, 1.0),
    key(0x41, 9.5, R0, 1.0),
    key(0x42, 11.0, R0, 1.0),
    key(0x43, 12.0, R0, 1.0),
    key(0x44, 13.0, R0, 1.0),
    key(0x45, 14.0, R0, 1.0),
    key(0x46, 15.25, R0, 1.0), // Print Screen
    key(0x47, 16.25, R0, 1.0), // Scroll Lock
    key(0x48, 17.25, R0, 1.0), // Pause
    // Number row.
    key(0x35, 0.0, R1, 1.0), // `
    key(0x1E, 1.0, R1, 1.0),
    key(0x1F, 2.0, R1, 1.0),
    key(0x20, 3.0, R1, 1.0),
    key(0x21, 4.0, R1, 1.0),
    key(0x22, 5.0, R1, 1.0),
    key(0x23, 6.0, R1, 1.0),
    key(0x24, 7.0, R1, 1.0),
    key(0x25, 8.0, R1, 1.0),
    key(0x26, 9.0, R1, 1.0),
    key(0x27, 10.0, R1, 1.0),
    key(0x2D, 11.0, R1, 1.0), // -
    key(0x2E, 12.0, R1, 1.0), // =
    key(0x2A, 13.0, R1, 2.0), // Backspace
    key(0x49, 15.25, R1, 1.0), // Insert
    key(0x4A, 16.25, R1, 1.0), // Home
    key(0x4B, 17.25, R1, 1.0), // Page Up
    // Top letter row.
    key(0x2B, 0.0, R2, 1.5), // Tab
    key(0x14, 1.5, R2, 1.0), // Q
    key(0x1A, 2.5, R2, 1.0), // W
    key(0x08, 3.5, R2, 1.0), // E
    key(0x15, 4.5, R2, 1.0), // R
    key(0x17, 5.5, R2, 1.0), // T
    key(0x1C, 6.5, R2, 1.0), // Y
    key(0x18, 7.5, R2, 1.0), // U
    key(0x0C, 8.5, R2, 1.0), // I
    key(0x12, 9.5, R2, 1.0), // O
    key(0x13, 10.5, R2, 1.0), // P
    key(0x2F, 11.5, R2, 1.0), // [
    key(0x30, 12.5, R2, 1.0), // ]
    Key { code: 0x28, x: 13.5, y: R2, w: 1.5, h: 2.0 }, // ISO Enter
    key(0x4C, 15.25, R2, 1.0), // Delete
    key(0x4D, 16.25, R2, 1.0), // End
    key(0x4E, 17.25, R2, 1.0), // Page Down
    // Home row.
    key(0x39, 0.0, R3, 1.75), // Caps Lock
    key(0x04, 1.75, R3, 1.0), // A
    key(0x16, 2.75, R3, 1.0), // S
    key(0x07, 3.75, R3, 1.0), // D
    key(0x09, 4.75, R3, 1.0), // F
    key(0x0A, 5.75, R3, 1.0), // G
    key(0x0B, 6.75, R3, 1.0), // H
    key(0x0D, 7.75, R3, 1.0), // J
    key(0x0E, 8.75, R3, 1.0), // K
    key(0x0F, 9.75, R3, 1.0), // L
    key(0x33, 10.75, R3, 1.0), // ;
    key(0x34, 11.75, R3, 1.0), // '
    key(0x32, 12.75, R3, 1.0), // #
    // Bottom letter row.
    key(0xE1, 0.0, R4, 1.25), // Left Shift
    key(0x64, 1.25, R4, 1.0), // \
    key(0x1D, 2.25, R4, 1.0), // Z
    key(0x1B, 3.25, R4, 1.0), // X
    key(0x06, 4.25, R4, 1.0), // C
    key(0x19, 5.25, R4, 1.0), // V
    key(0x05, 6.25, R4, 1.0), // B
    key(0x11, 7.25, R4, 1.0), // N
    key(0x10, 8.25, R4, 1.0), // M
    key(0x36, 9.25, R4, 1.0), // ,
    key(0x37, 10.25, R4, 1.0), // .
    key(0x38, 11.25, R4, 1.0), // /
    key(0xE5, 12.25, R4, 2.75), // Right Shift
    key(0x52, 16.25, R4, 1.0), // Up
    // Modifier row.
    key(0xE0, 0.0, R5, 1.25), // Left Ctrl
    key(0xE3, 1.25, R5, 1.25), // Left Win
    key(0xE2, 2.5, R5, 1.25), // Left Alt
    key(0x2C, 3.75, R5, 6.25), // Space
    key(0xE6, 10.0, R5, 1.25), // AltGr
    key(0x65, 12.5, R5, 1.25), // Menu
    key(0xE4, 13.75, R5, 1.25), // Right Ctrl
    key(0x50, 15.25, R5, 1.0), // Left
    key(0x51, 16.25, R5, 1.0), // Down
    key(0x4F, 17.25, R5, 1.0), // Right
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_inside_the_board() {
        let mut seen = [false; 256];
        for k in &ISO_UK {
            assert!(!seen[usize::from(k.code)], "duplicate code {:#04x}", k.code);
            seen[usize::from(k.code)] = true;
            assert!(k.x >= 0.0 && k.x + k.w <= WIDTH);
            assert!(k.y >= 0.0 && k.y + k.h <= HEIGHT);
        }
    }
}
