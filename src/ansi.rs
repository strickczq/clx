//! ANSI SGR styling helpers shared across the binary.
//!
//! SGR select-graphic-rendition codes.
//! See https://en.wikipedia.org/wiki/ANSI_escape_code#SGR_parameters

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const RED: u8 = 91;
pub const GREEN: u8 = 92;
pub const YELLOW: u8 = 93;
pub const CYAN: u8 = 96;

/// Wrap `text` in an SGR style/colour code, resetting style afterwards.
pub fn paint(code: u8, text: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// Like [`paint`], but returns the text unchanged when `enabled` is false —
/// used to skip colour when the output stream is not a terminal.
pub fn paint_if(code: u8, text: &str, enabled: bool) -> String {
    if enabled {
        paint(code, text)
    } else {
        text.to_string()
    }
}
