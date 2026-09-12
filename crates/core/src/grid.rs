//! Cell buffer model: memori-efisien, tanpa Spannable.

/// Atribut teks per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub fg: Color,
    pub bg: Color,
}

impl CellAttrs {
    pub fn is_plain(self) -> bool {
        self == CellAttrs::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Satu sel terminal. `width` untuk karakter wide (CJK/emoji).
#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct Grid {
    lines: Vec<Line>,
    cols: usize,
    rows: usize,
    scrollback: Scrollback,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, scrollback_cap: usize) -> Self {
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
        &self.lines[y]
    }

    fn line_mut(&mut self, y: usize) -> &mut Line {
        &mut self.lines[y]
    }

    /// Tulis cell di (x,y). Return lebar char (untuk wide chars).
    pub fn set(&mut self, x: usize, y: usize, cell: Cell) -> usize {
        let w = cell.width as usize;
        let cols = self.cols;
        let line = self.line_mut(y);
        line.cells[x.min(cols - 1)] = cell;
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
#[derive(Debug, Clone)]
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
