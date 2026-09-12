//! Cell buffer model: memori-efisien, tanpa Spannable.

use serde::{Deserialize, Serialize};

/// Atribut teks per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellAttrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub fg: Color,
    pub bg: Color,
    /// ID hyperlink OSC 8 di registry terminal; `None` = bukan link.
    pub hyperlink: Option<u32>,
    /// ID kitty image yang menutupi cell; `None` = teks biasa.
    pub image: Option<u32>,
}

impl CellAttrs {
    pub fn is_plain(self) -> bool {
        self == CellAttrs::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// Resolve ke RGB 24-bit. `Default` → None (biarkan host pakai warna default).
    pub fn to_rgb24(self) -> Option<(u8, u8, u8)> {
        match self {
            Color::Default => None,
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Indexed(i) => Some(indexed_to_rgb(i)),
        }
    }
}

/// Peta 256-color xterm: 0-15 sistem, 16-231 cube 6x6x6, 232-255 grayscale.
pub fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match i {
        0..=15 => SYSTEM[i as usize],
        16..=231 => {
            let n = i - 16;
            const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let r = CUBE[(n / 36) as usize];
            let g = CUBE[(n / 6 % 6) as usize];
            let b = CUBE[(n % 6) as usize];
            (r, g, b)
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

/// Satu sel terminal. `width` untuk karakter wide (CJK/emoji).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    pub attrs: CellAttrs,
    pub width: u8,
}

impl Cell {
    pub fn new(ch: char, attrs: CellAttrs) -> Self {
        Cell {
            ch,
            attrs,
            width: text_width(ch),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ch == ' '
    }
}

/// Lebar display sebuah karakter (naih cost; cache via lookup selanjutnya).
pub fn text_width(ch: char) -> u8 {
    if ch == '\u{0}' {
        return 1;
    }
    let cp = ch as u32;
    if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE4F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1FAFF).contains(&cp)
    {
        2
    } else {
        1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub cells: Vec<Cell>,
    pub dirty: bool,
}

impl Line {
    pub fn new(cols: usize) -> Self {
        Line {
            cells: vec![Cell::new(' ', CellAttrs::default()); cols],
            dirty: true,
        }
    }
}

/// Buffer layar utama: `rows` Line x `cols` cell + scrollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grid {
    lines: Vec<Line>,
    cols: usize,
    rows: usize,
    scrollback: Scrollback,
}

impl Default for Grid {
    fn default() -> Self {
        Grid::new(1, 1, 0)
    }
}

impl Grid {
    pub fn new(cols: usize, rows: usize, scrollback_cap: usize) -> Self {
        // clamp >= 1 supaya `cols - 1` / index tak pernah underflow
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut lines = Vec::with_capacity(rows);
        for _ in 0..rows {
            lines.push(Line::new(cols));
        }
        Grid {
            lines,
            cols,
            rows,
            scrollback: Scrollback::new(scrollback_cap),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn line(&self, y: usize) -> &Line {
        &self.lines[y.min(self.rows.saturating_sub(1))]
    }

    /// Baris untuk viewport yang di-scroll `offset` baris ke atas dari bawah.
    /// `offset == 0` = ikut bottom (baris layar utama). Saat scrolled,
    /// baris diambil dari scrollback lalu layar utama (chronologis).
    pub fn view_line(&self, y: usize, offset: usize) -> &Line {
        let sb = self.scrollback.len();
        let k = offset.min(sb);
        if k == 0 {
            return self.line(y);
        }
        let idx = sb - k + y; // mulai dari sb-k; idx >= sb → layar utama
        if idx < sb {
            self.scrollback.get(idx)
        } else {
            &self.lines[(idx - sb).min(self.rows.saturating_sub(1))]
        }
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Akses cell mutable; `None` kalau x di luar kolom (y di-clamp).
    pub fn cell_at_mut(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        if x >= self.cols {
            return None;
        }
        let line = self.line_mut(y.min(self.rows.saturating_sub(1)));
        Some(&mut line.cells[x])
    }

    fn line_mut(&mut self, y: usize) -> &mut Line {
        &mut self.lines[y]
    }

    /// Tulis cell di (x,y). Return lebar char (untuk wide chars).
    pub fn set(&mut self, x: usize, y: usize, cell: Cell) -> usize {
        let w = cell.width as usize;
        let cols = self.cols;
        let line = self.line_mut(y.min(self.rows.saturating_sub(1)));
        line.cells[x.min(cols.saturating_sub(1))] = cell;
        line.dirty = true;
        // placeholder cell kalau wide
        if w == 2 && x + 1 < cols {
            line.cells[x + 1] = Cell::new(' ', CellAttrs::default());
        }
        w
    }

    pub fn clear_line(&mut self, y: usize) {
        let line = self.line_mut(y);
        for c in line.cells.iter_mut() {
            *c = Cell::new(' ', CellAttrs::default());
        }
        line.dirty = true;
    }

    pub fn clear_range(&mut self, y: usize, from: usize, to: usize) {
        let cols = self.cols;
        let line = self.line_mut(y);
        let lo = from.min(cols);
        let hi = to.min(cols);
        for c in line.cells[lo..hi].iter_mut() {
            *c = Cell::new(' ', CellAttrs::default());
        }
        line.dirty = true;
    }

    /// Scroll ke atas n baris; baris teratas masuk scrollback.
    pub fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let moved = self.lines.remove(0);
            self.scrollback.push(moved);
            let fresh = Line::new(self.cols);
            self.lines.push(fresh);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        for line in self.lines.iter_mut() {
            line.cells
                .resize(cols, Cell::new(' ', CellAttrs::default()));
            line.dirty = true;
        }
        while self.lines.len() < rows {
            self.lines.push(Line::new(cols));
        }
        self.lines.truncate(rows);
    }
}

/// Ring buffer scrollback: `Vec` + offset, tanpa shifting O(n).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scrollback {
    lines: Vec<Line>,
    start: usize,
    cap: usize,
}

impl Scrollback {
    pub fn new(cap_entries: usize) -> Self {
        Scrollback {
            lines: Vec::with_capacity(cap_entries),
            start: 0,
            cap: cap_entries,
        }
    }

    pub fn push(&mut self, line: Line) {
        if self.lines.len() == self.cap {
            self.lines.remove(0);
        } else if self.start > 0 {
            self.start -= 1;
            self.lines.insert(0, line);
            return;
        }
        self.lines.push(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn get(&self, i: usize) -> &Line {
        &self.lines[i.min(self.lines.len().saturating_sub(1))]
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_width_handles_wide() {
        assert_eq!(text_width('a'), 1);
        assert_eq!(text_width('中'), 2);
        assert_eq!(text_width('🚀'), 2);
    }

    #[test]
    fn scrollback_ring_respects_cap() {
        let mut sb = Scrollback::new(3);
        for _ in 0..5 {
            sb.push(Line::new(1));
        }
        assert_eq!(sb.len(), 3);
    }

    #[test]
    fn cell_default_is_empty() {
        let c = Cell::new(' ', CellAttrs::default());
        assert!(c.is_empty());
    }
}
