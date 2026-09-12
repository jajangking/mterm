//! mterm-core: terminal engine.
//!
//! Modul ini memilik tiga lapisan:
//! - `grid`: model buffer (Cell, Line, Grid, Scrollback)
//! - `terminal`: state machine yang menggerakkan grid dari event VT
//! - `parser`: adapter di atas crate `vte` yang memetakan byte → tindakan

pub mod grid;
pub mod terminal;

pub use grid::{Cell, CellAttrs, Color, Grid, Scrollback};
pub use terminal::{Terminal, TerminalConfig};
