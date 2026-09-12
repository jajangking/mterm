//! mterm-core: terminal engine.
//!
//! Modul ini memilik tiga lapisan:
//! - `grid`: model buffer (Cell, Line, Grid, Scrollback)
//! - `terminal`: state machine yang menggerakkan grid dari event VT
//! - `parser`: adapter di atas crate `vte` yang memetakan byte → tindakan

pub mod grid;
pub mod mouse;
pub mod terminal;

pub use grid::{Cell, CellAttrs, Color, Grid, Scrollback};
pub use mouse::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, MOD_ALT, MOD_CTRL, MOD_SHIFT};
pub use terminal::{Terminal, TerminalConfig};
