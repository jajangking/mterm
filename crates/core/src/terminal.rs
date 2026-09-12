//! Terminal state machine: menggerakkan `Grid` dari event VT.

use crate::grid::{Cell, CellAttrs, Color, Grid};
use vte::{Params, Perform};

#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub cols: usize,
    pub rows: usize,
    pub scrollback_cap: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            cols: 80,
            rows: 24,
            scrollback_cap: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Wrap,
    AlternateScreen,
    BracketedPaste,
}

#[derive(Debug, Clone)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    pub saved_x: usize,
    pub saved_y: usize,
    pub attrs: CellAttrs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Title(String),
    Bell,
    Hyperlink(Option<String>),
    Mouse(bool),
}

/// Implementasi `vte::Perform`: byte dari PTY → aksi ke grid.
pub struct Terminal {
    pub config: TerminalConfig,
    pub grid: Grid,
    pub cursor: Cursor,
    pub mode: Mode,
    pub main_screen: Grid,
    pub alt_screen: Option<Grid>,
    pub pending_title: Option<String>,
    pub dirty_rect: Option<(usize, usize, usize, usize)>,
}

impl Terminal {
    pub fn new(config: TerminalConfig) -> Self {
        let grid = Grid::new(config.cols, config.rows, config.scrollback_cap);
        let cursor = Cursor {
            x: 0,
            y: 0,
            saved_x: 0,
            saved_y: 0,
            attrs: CellAttrs::default(),
        };
        Terminal {
            config,
            grid,
            cursor,
            mode: Mode::Wrap,
            main_screen: Grid::new(0, 0, 0),
            alt_screen: None,
            pending_title: None,
            dirty_rect: None,
        }
    }

    pub fn cols(&self) -> usize {
        self.config.cols
    }
    pub fn rows(&self) -> usize {
        self.config.rows
    }

    fn put_char(&mut self, ch: char) {
        if ch == '\r' {
            self.cursor.x = 0;
            return;
        }
        if ch == '\n' {
            self.cursor.y = (self.cursor.y + 1).min(self.rows());
            self.maybe_scroll();
            return;
        }
        if self.cursor.x >= self.cols() {
            // wrap SEBELUM menulis (kursor di kolom terakhir)
            if self.mode == Mode::Wrap {
                self.cursor.y += 1;
                self.maybe_scroll();
                self.cursor.x = 0;
            } else {
                self.cursor.x = self.cols() - 1;
            }
        }
        let attrs = self.cursor.attrs;
        self.cursor.x += self
            .grid
            .set(self.cursor.x, self.cursor.y, Cell::new(ch, attrs));
    }

    fn maybe_scroll(&mut self) {
        if self.cursor.y >= self.rows() {
            self.cursor.y = self.rows() - 1;
            self.grid.scroll_up(1);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.config.cols = cols;
        self.config.rows = rows;
        self.grid.resize(cols, rows);
        self.cursor.x = self.cursor.x.min(cols.saturating_sub(1));
        self.cursor.y = self.cursor.y.min(rows.saturating_sub(1));
    }

    pub fn set_title(&mut self, title: String) {
        self.pending_title = Some(title);
    }

    /// Feed byte mentah (output PTY/socket) → parser + engine.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let mut parser = vte::Parser::new();
        parser.advance(self, bytes);
    }

    /// Feed string/lossy utf8.
    pub fn feed_str(&mut self, s: &str) {
        self.feed_bytes(s.as_bytes());
    }

    pub fn take_event(&mut self) -> Option<TerminalEvent> {
        self.pending_title.take().map(TerminalEvent::Title)
    }
}

impl Perform for Terminal {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
        self.mark_dirty(self.cursor.x, self.cursor.y);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\x07' => self.pending_title = None,
            b'\r' => self.cursor.x = 0,
            b'\n' => {
                self.cursor.y = (self.cursor.y + 1).min(self.rows());
                self.maybe_scroll();
            }
            b'\t' => {
                let tab = 8;
                let next = ((self.cursor.x / tab) + 1) * tab;
                self.cursor.x = next.min(self.cols() - 1);
            }
            b'\x08' if self.cursor.x > 0 => {
                self.cursor.x -= 1;
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, inter: &[u8], ignore: bool, c: char) {
        if !inter.is_empty() || ignore {
            return;
        }
        let list = param_list(params);
        let p = |i: usize, d: u16| list.get(i).copied().filter(|v| *v > 0).unwrap_or(d);
        match c {
            'A' => self.cursor.y = self.cursor.y.saturating_sub(p(0, 1) as usize),
            'B' => self.cursor.y = (self.cursor.y + p(0, 1) as usize).min(self.rows() - 1),
            'C' => self.cursor.x = (self.cursor.x + p(0, 1) as usize).min(self.cols() - 1),
            'D' => self.cursor.x = self.cursor.x.saturating_sub(p(0, 1) as usize),
            'G' => self.cursor.x = (p(0, 1) as usize - 1).min(self.cols() - 1),
            'd' => self.cursor.y = (p(0, 1) as usize - 1).min(self.rows() - 1),
            'H' | 'f' => {
                self.cursor.y = (p(0, 1) as usize - 1).min(self.rows() - 1);
                self.cursor.x = (p(1, 1) as usize - 1).min(self.cols() - 1);
            }
            'J' => match p(0, 0) {
                0 => {
                    for y in self.cursor.y..self.rows() {
                        self.grid.clear_line(y);
                    }
                }
                2 => {
                    for y in 0..self.rows() {
                        self.grid.clear_line(y);
                    }
                    self.cursor.x = 0;
                    self.cursor.y = 0;
                }
                _ => {}
            },
            'K' => self
                .grid
                .clear_range(self.cursor.y, self.cursor.x, self.cols()),
            'm' => {
                let list = param_list(params);
                let mut i = 0usize;
                while i < list.len() {
                    let v = list[i];
                    match v {
                        38 | 48 => {
                            let color = self.parse_color(&list, i);
                            if let Some(c) = color {
                                if v == 38 {
                                    self.cursor.attrs.fg = c;
                                } else {
                                    self.cursor.attrs.bg = c;
                                }
                            }
                            i += color_param_consumed(&list, i).max(1);
                            continue;
                        }
                        _ => {
                            self.apply_sgr(v);
                            i += 1;
                        }
                    }
                }
            }
            'h' => {
                if let Some(v) = list.first().copied() {
                    match v {
                        1049 | 47 if self.alt_screen.is_none() => {
                            let (cols, rows) = (self.cols(), self.rows());
                            self.alt_screen =
                                Some(std::mem::replace(&mut self.grid, Grid::new(cols, rows, 0)));
                        }
                        _ => {}
                    }
                }
            }
            'l' => {
                if let Some(1049 | 47) = list.first().copied() {
                    if let Some(alt) = self.alt_screen.take() {
                        self.grid = alt;
                    }
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let _ = bell_terminated;
        if params.is_empty() {
            return;
        }
        let code = params[0];
        if code == b"0" || code == b"2" {
            if let Some(title) = params.get(1) {
                if let Ok(t) = std::str::from_utf8(title) {
                    self.set_title(t.to_string());
                }
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, c: char) {
        let _ = c;
    }

    fn unhook(&mut self) {}
    fn put(&mut self, _byte: u8) {}
}

impl Terminal {
    fn apply_sgr(&mut self, val: u16) {
        let a = &mut self.cursor.attrs;
        match val {
            0 => *a = CellAttrs::default(),
            1 => a.bold = true,
            3 => a.italic = true,
            4 => a.underline = true,
            9 => a.strikethrough = true,
            7 => a.inverse = true,
            22 => {
                a.bold = false;
                a.italic = false;
            }
            24 => a.underline = false,
            27 => a.inverse = false,
            30..=37 => a.fg = Color::Indexed((val - 30) as u8),
            39 => a.fg = Color::Default,
            40..=47 => a.bg = Color::Indexed((val - 40) as u8),
            49 => a.bg = Color::Default,
            90..=97 => a.fg = Color::Indexed((val - 90 + 8) as u8),
            100..=107 => a.bg = Color::Indexed((val - 100 + 8) as u8),
            _ => {}
        }
    }

    /// Parse `38;5;n` / `38;2;r;g;b` pada list param mulai index `i` (38/48).
    fn parse_color(&self, list: &[u16], i: usize) -> Option<Color> {
        match list.get(i + 1) {
            Some(5) => list
                .get(i + 2)
                .copied()
                .map(|idx| Color::Indexed(idx as u8)),
            Some(2) => {
                let r = *list.get(i + 2)? as u8;
                let g = *list.get(i + 3)? as u8;
                let b = *list.get(i + 4)? as u8;
                Some(Color::Rgb(r, g, b))
            }
            _ => None,
        }
    }

    fn mark_dirty(&mut self, x: usize, y: usize) {
        self.dirty_rect = Some(match self.dirty_rect {
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            None => (x, y, x, y),
        });
    }
}

/// Flatten `Params` (subparameter slices) ke Vec<u16>.
fn param_list(params: &Params) -> Vec<u16> {
    let mut out = Vec::with_capacity(params.len());
    for sub in params.iter() {
        out.extend_from_slice(sub);
    }
    out
}

/// Jumlah param yang dikonsumsi oleh SGR 38/48 mulai index i (termasuk 38/48).
fn color_param_consumed(list: &[u16], i: usize) -> usize {
    match list.get(i + 1) {
        Some(5) => 3,
        Some(2) => 5,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_2x2() -> Terminal {
        Terminal::new(TerminalConfig {
            cols: 2,
            rows: 2,
            scrollback_cap: 10,
        })
    }

    fn feed(t: &mut Terminal, s: &str) {
        let mut parser = vte::Parser::new();
        parser.advance(t, s.as_bytes());
    }

    #[test]
    fn prints_chars_and_wraps() {
        let mut t = term_2x2();
        feed(&mut t, "abcd");
        assert_eq!(t.grid.line(0).cells[0].ch, 'a');
        assert_eq!(t.grid.line(0).cells[1].ch, 'b');
        assert_eq!(t.grid.line(1).cells[0].ch, 'c');
        assert_eq!(t.grid.line(1).cells[1].ch, 'd');
    }

    #[test]
    fn cursor_movement_csi() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b[2;1HX");
        assert_eq!(t.grid.line(1).cells[0].ch, 'X');
    }

    #[test]
    fn clear_screen_j2() {
        let mut t = term_2x2();
        feed(&mut t, "ab");
        feed(&mut t, "\x1b[2J");
        for y in 0..2 {
            for x in 0..2 {
                assert!(t.grid.line(y).cells[x].is_empty());
            }
        }
    }

    #[test]
    fn title_via_osc() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b]0;hello\x07");
        assert_eq!(t.take_event(), Some(TerminalEvent::Title("hello".into())));
    }

    #[test]
    fn big_scroll_does_not_panic() {
        let mut t = term_2x2();
        let mut buf = String::new();
        for _ in 0..10_000 {
            buf.push_str("line\n");
        }
        feed(&mut t, &buf);
        assert_eq!(t.rows(), 2);
    }
}
