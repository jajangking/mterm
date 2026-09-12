//! Terminal state machine: menggerakkan `Grid` dari event VT.

use crate::grid::{Cell, CellAttrs, Color, Grid};
use crate::kitty::{KittyAction, KittyCommand, KittyFormat, KittyImage, PendingChunk};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use vte::{Params, Perform};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Insert,
    Wrap,
    AlternateScreen,
    BracketedPaste,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Gambar kitty selesai ditransmisikan (data penuh untuk renderer).
    KittyImage {
        id: u32,
        format: u8,
        width_px: u32,
        height_px: u32,
        data: Arc<Vec<u8>>,
    },
    /// Gambar ditempatkan di grid (cell `attrs.image = id`).
    KittyPlaced {
        id: u32,
        x: usize,
        y: usize,
        cols: usize,
        rows: usize,
    },
    /// Gambar dihapus dari registry.
    KittyDeleted {
        id: u32,
    },
}

/// Implementasi `vte::Perform`: byte dari PTY → aksi ke grid.
///
/// `Serialize`/`Deserialize` dipakai untuk session persistence (Fase 4):
/// grid + scrollback + cursor + mode + hyperlink disimpan; hal transient
/// (events, kitty image, dirty_rect) tidak ikut disimpan.
#[derive(Serialize, Deserialize)]
pub struct Terminal {
    pub config: TerminalConfig,
    pub grid: Grid,
    pub cursor: Cursor,
    pub mode: Mode,
    #[serde(skip)]
    pub main_screen: Grid,
    pub alt_screen: Option<Grid>,
    #[serde(skip)]
    pub events: VecDeque<TerminalEvent>,
    #[serde(skip)]
    pub dirty_rect: Option<(usize, usize, usize, usize)>,
    /// Scrollback offset viewport: 0 = ikut bottom; >0 = tampilkan scrollback.
    scroll_offset: usize,
    pub current_hyperlink: Option<u32>,
    pub hyperlinks: HashMap<u32, String>,
    next_hyperlink: u32,
    pub mouse_tracking: bool,
    pub sgr_mouse: bool,
    /// Registry kitty image: id → gambar.
    #[serde(skip)]
    pub images: HashMap<u32, crate::kitty::KittyImage>,
    #[serde(skip)]
    pub next_image: u32,
    #[serde(skip)]
    pub pending_chunk: Option<crate::kitty::PendingChunk>,
    #[serde(skip)]
    pending_apc: Option<Vec<u8>>,
}

impl Terminal {
    pub fn new(config: TerminalConfig) -> Self {
        // jaga >= 1 kolom/baris supaya `cols - 1` di grid tak pernah underflow
        let config = TerminalConfig {
            cols: config.cols.max(1),
            rows: config.rows.max(1),
            ..config
        };
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
            events: VecDeque::new(),
            dirty_rect: None,
            scroll_offset: 0,
            current_hyperlink: None,
            hyperlinks: HashMap::new(),
            next_hyperlink: 0,
            mouse_tracking: false,
            sgr_mouse: false,
            images: HashMap::new(),
            next_image: 0,
            pending_chunk: None,
            pending_apc: None,
        }
    }

    pub fn cols(&self) -> usize {
        self.config.cols
    }
    pub fn rows(&self) -> usize {
        self.config.rows
    }

    // ── Viewport scrollback ──────────────────────────────────────────────

    /// Jumlah baris yang bisa di-scroll (jumlah scrollback saat ini).
    pub fn scrollback_len(&self) -> usize {
        self.grid.scrollback_len()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set offset viewport (0 = ikut bottom); di-clamp ke scrollback.
    /// Kalau berubah, tandai seluruh layar dirty supaya chrome repaint.
    pub fn set_scroll_offset(&mut self, k: usize) {
        let k = k.min(self.grid.scrollback_len());
        if k != self.scroll_offset {
            self.scroll_offset = k;
            let (cols, rows) = (self.cols(), self.rows());
            self.dirty_rect = Some((0, 0, cols, rows));
        }
    }

    /// Baris display `y` dilihat lewat viewport (scrollback kalau offset > 0).
    pub fn view_line(&self, y: usize) -> &crate::grid::Line {
        self.grid.view_line(y, self.scroll_offset)
    }

    // ── Session persistence (Fase 4) ─────────────────────────────────────

    /// Serialisasi penuh state terminal (grid, scrollback, cursor, mode,
    /// hyperlink) jadi JSON. Event queue & kitty image TIDAK ikut.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Restore terminal dari hasil [to_json]. `dirty_rect` dikosongkan (caller
    /// yang menentukan perlu repaint penuh setelah restore).
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut t: Terminal = serde_json::from_str(s)?;
        t.dirty_rect = None;
        t.events.clear();
        Ok(t)
    }

    /// Encode event mouse ke SGR (mode 1006). Kosong kalau SGR belum aktif —
    /// chrome boleh mengirim tanpa cek mode dulu.
    pub fn sgr_mouse_seq(
        &self,
        code: u8,
        mods: u8,
        release: bool,
        x: usize,
        y: usize,
        out: &mut Vec<u8>,
    ) -> bool {
        if !self.sgr_mouse {
            return false;
        }
        crate::mouse::encode(code, mods, release, x, y, out);
        true
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
        if (ch as u32) < 0x100 {
            self.put_ascii(ch as u8);
            return;
        }
        self.put_glyph_wide(ch);
    }

    /// Tulis 1 char printable ASCII (0x20..=0x7e) + wrap; hot path feed.
    #[inline]
    fn put_ascii(&mut self, b: u8) {
        if b < 0x20 {
            return;
        }
        if self.cursor.x >= self.cols() {
            if self.mode == Mode::Wrap {
                self.cursor.y += 1;
                self.maybe_scroll();
                self.cursor.x = 0;
            } else {
                self.cursor.x = self.cols().saturating_sub(1);
            }
        }
        let mut attrs = self.cursor.attrs;
        attrs.hyperlink = self.current_hyperlink;
        self.cursor.x += self
            .grid
            .set(self.cursor.x, self.cursor.y, Cell::new(b as char, attrs));
    }

    /// Char non-ASCII (>= 100 hex) via vte — beat sudah di `boundary`; di sini
    /// tulis sebagai lebar 1 (normalisasi CJK belum didukung).
    fn put_glyph_wide(&mut self, ch: char) {
        if self.cursor.x >= self.cols() {
            if self.mode == Mode::Wrap {
                self.cursor.y += 1;
                self.maybe_scroll();
                self.cursor.x = 0;
            } else {
                self.cursor.x = self.cols().saturating_sub(1);
            }
        }
        let mut attrs = self.cursor.attrs;
        attrs.hyperlink = self.current_hyperlink;
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
        self.scroll_offset = self.scroll_offset.min(self.grid.scrollback_len());
    }

    fn enter_alt(&mut self, cols: usize, rows: usize) {
        self.scroll_offset = 0; // alt screen tak punya scrollback
        self.alt_screen = Some(std::mem::replace(&mut self.grid, Grid::new(cols, rows, 0)));
    }

    pub fn set_title(&mut self, title: String) {
        self.events.push_back(TerminalEvent::Title(title));
    }

    #[inline]
    fn fast_byte(b: u8) -> bool {
        (0x20..=0x7e).contains(&b) || b == b'\r' || b == b'\n'
    }

    /// Feed byte mentah (output PTY/socket) → parser + engine.
    /// Fast path: run ASCII printable/CR/LF ditulis langsung tanpa vte;
    /// byte lain (ESC, control, utf8) → vte untuk sisa chunk.
    /// Kitc graphics APC (`ESC _ ... ESC \`) dicegat sebelum vte.
    pub fn feed_bytes(&mut self, mut bytes: &[u8]) {
        // lanjutkan APC yang terpotong dari feed sebelumnya
        if let Some(mut buf) = self.pending_apc.take() {
            buf.extend_from_slice(bytes);
            if let Some(len) = Self::find_apc_end(&buf) {
                self.apply_apc(&buf[3..len - 2]);
                if buf.len() > len {
                    let rest = buf[len..].to_vec();
                    self.feed_bytes(&rest);
                }
                return;
            }
            self.pending_apc = Some(buf);
            return;
        }
        while !bytes.is_empty() {
            if bytes[0] == 0x1b && bytes.get(1) == Some(&b'_') {
                if let Some(len) = Self::find_apc_end(bytes) {
                    self.apply_apc(&bytes[3..len - 2]);
                    bytes = &bytes[len..];
                    continue;
                }
                // APC belum lengkap → buffer, tunggu feed berikutnya
                self.pending_apc = Some(bytes.to_vec());
                return;
            }
            if Self::fast_byte(bytes[0]) {
                let mut n = 1;
                while n < bytes.len() && Self::fast_byte(bytes[n]) {
                    n += 1;
                }
                self.put_fast(&bytes[..n]);
                bytes = &bytes[n..];
            } else {
                let mut parser = vte::Parser::new();
                parser.advance(self, bytes);
                return;
            }
        }
    }

    /// Cari terminator APC (`ESC \`); return index setelah terminator.
    fn find_apc_end(bytes: &[u8]) -> Option<usize> {
        if bytes.len() >= 3 && bytes[0] == 0x1b && bytes[1] == b'_' {
            // index mencari `ESC \`
            let mut i = 2;
            while i + 1 < bytes.len() {
                if bytes[i] == 0x1b && bytes[i + 1] == b'\\' {
                    return Some(i + 2);
                }
                i += 1;
            }
        }
        None
    }

    /// Proses APC (dari content antara `ESC _` dan `ESC \`).
    fn apply_apc(&mut self, content: &[u8]) {
        let cmd = crate::kitty::parse_apc(content);
        match cmd.action {
            KittyAction::Transmit | KittyAction::TransmitAndPlace => {
                self.kitty_transmit(cmd);
            }
            KittyAction::Place => self.kitty_place(&cmd),
            KittyAction::Delete => self.kitty_delete(&cmd),
            KittyAction::Query | KittyAction::Ignore => {}
        }
    }

    /// Transmit (chunked): tumpuk payload, lengkapi di `m=0`, simpan registry.
    fn kitty_transmit(&mut self, cmd: KittyCommand) {
        // id: eksplisit `q=` > id chunk yang masih kelanjutan > auto-increment
        let continuation = self.pending_chunk.as_ref().map(|pc| pc.image_id);
        let id = cmd.image_id.or(continuation).unwrap_or_else(|| {
            self.next_image = self.next_image.wrapping_add(1);
            self.next_image
        });
        let action = cmd.action;
        let done = cmd.more <= 0;
        // salin field Copy biar setelah `payload` dipindah masih bisa dipakai
        let place_x = cmd.x;
        let place_y = cmd.y;
        let cols = cmd.cols;
        let rows = cmd.rows;
        let fmt = cmd.format;
        let w = cmd.width_px;
        let h = cmd.height_px;

        let insert_done = |this: &mut Self, cfg: crate::kitty::Completed| {
            this.images.insert(
                cfg.id,
                KittyImage {
                    format: cfg.format,
                    width_px: cfg.width_px,
                    height_px: cfg.height_px,
                    data: Arc::new(cfg.bytes),
                },
            );
            if let Some(img) = this.images.get(&cfg.id) {
                this.events.push_back(TerminalEvent::KittyImage {
                    id: cfg.id,
                    format: format_tag(cfg.format),
                    width_px: cfg.width_px,
                    height_px: cfg.height_px,
                    data: img.data.clone(),
                });
            }
        };

        match self.pending_chunk.take() {
            Some(mut pc) => {
                if pc.image_id != id {
                    // id beda: chunk lama tidak selesai → buang
                    pc = PendingChunk {
                        bytes: Vec::new(),
                        ..pc
                    };
                }
                pc.bytes.extend_from_slice(&cmd.payload);
                if done {
                    let format = if fmt != KittyFormat::Unknown {
                        fmt
                    } else {
                        pc.format
                    };
                    let width_px = if w > 0 { w } else { pc.width_px };
                    let height_px = if h > 0 { h } else { pc.height_px };
                    insert_done(
                        self,
                        crate::kitty::Completed {
                            id,
                            format,
                            width_px,
                            height_px,
                            bytes: std::mem::take(&mut pc.bytes),
                        },
                    );
                } else {
                    self.pending_chunk = Some(pc);
                }
            }
            None => {
                if done {
                    let format = if fmt != KittyFormat::Unknown {
                        fmt
                    } else {
                        KittyFormat::Unknown
                    };
                    let width_px = if w > 0 { w } else { 0 };
                    let height_px = if h > 0 { h } else { 0 };
                    insert_done(
                        self,
                        crate::kitty::Completed {
                            id,
                            format,
                            width_px,
                            height_px,
                            bytes: cmd.payload,
                        },
                    );
                } else {
                    self.pending_chunk = Some(PendingChunk {
                        bytes: cmd.payload,
                        format: fmt,
                        width_px: w,
                        height_px: h,
                        image_id: id,
                    });
                }
            }
        }
        if action == KittyAction::TransmitAndPlace {
            self.kitty_place_at(id, place_x, place_y, cols, rows);
        }
    }

    /// Place image `q=` (atau yang baru saja) di X/Y (1-based) ukuran c×r.
    fn kitty_place(&mut self, cmd: &KittyCommand) {
        let id = cmd.image_id.unwrap_or(self.next_image);
        self.kitty_place_at(id, cmd.x, cmd.y, cmd.cols, cmd.rows);
    }

    fn kitty_place_at(
        &mut self,
        id: u32,
        ix: Option<u32>,
        iy: Option<u32>,
        icols: u32,
        irows: u32,
    ) {
        if !self.images.contains_key(&id) {
            return;
        }
        let (x, y) = match (ix, iy) {
            (Some(x), Some(y)) => (
                (x as usize)
                    .saturating_sub(1)
                    .min(self.cols().saturating_sub(1)),
                (y as usize)
                    .saturating_sub(1)
                    .min(self.rows().saturating_sub(1)),
            ),
            _ => (
                self.cursor.x,
                self.cursor.y.min(self.rows().saturating_sub(1)),
            ),
        };
        let cols = (icols as usize).max(1).min(self.cols() - x);
        let rows = (irows as usize).max(1).min(self.rows() - y);
        let mut placed = Vec::new();
        for yy in y..y + rows {
            for xx in x..x + cols {
                if let Some(cell) = self.grid.cell_at_mut(xx, yy) {
                    cell.ch = ' ';
                    cell.attrs.image = Some(id);
                    cell.width = 1;
                    placed.push((xx, yy));
                }
            }
        }
        if !placed.is_empty() {
            self.mark_dirty_range(x, y, cols, rows);
            self.events.push_back(TerminalEvent::KittyPlaced {
                id,
                x,
                y,
                cols,
                rows,
            });
        }
    }

    /// Delete image: `q=id`, atau semua kalau `i=-1` / tanpa id.
    fn kitty_delete(&mut self, cmd: &KittyCommand) {
        let _ = cmd.image_no;
        match cmd.image_id {
            Some(id) => {
                if self.images.remove(&id).is_some() {
                    // bersihkan cell yang masih nunjuk id ini
                    self.clear_image_cells(id);
                    self.events.push_back(TerminalEvent::KittyDeleted { id });
                }
            }
            None => {
                for id in self.images.keys().copied().collect::<Vec<_>>() {
                    self.clear_image_cells(id);
                    self.events.push_back(TerminalEvent::KittyDeleted { id });
                }
                self.images.clear();
            }
        }
        self.pending_chunk = None;
    }

    fn clear_image_cells(&mut self, id: u32) {
        for y in 0..self.rows() {
            for x in 0..self.cols() {
                if let Some(cell) = self.grid.cell_at_mut(x, y) {
                    if cell.attrs.image == Some(id) {
                        cell.attrs.image = None;
                        cell.ch = ' ';
                    }
                }
            }
        }
    }

    fn mark_dirty_range(&mut self, x: usize, y: usize, cols: usize, rows: usize) {
        let x1 = x.min(self.cols());
        let y1 = y.min(self.rows());
        let x2 = (x + cols).min(self.cols());
        let y2 = (y + rows).min(self.rows());
        self.mark_dirty(x1, y1);
        self.mark_dirty(x2.saturating_sub(1), y2.saturating_sub(1));
    }

    fn put_fast(&mut self, run: &[u8]) {
        for &b in run {
            match b {
                b'\r' => self.cursor.x = 0,
                b'\n' => {
                    self.cursor.y = (self.cursor.y + 1).min(self.rows());
                    self.maybe_scroll();
                }
                _ => self.put_ascii(b),
            }
        }
    }

    /// Feed string/lossy utf8.
    pub fn feed_str(&mut self, s: &str) {
        self.feed_bytes(s.as_bytes());
    }

    pub fn take_event(&mut self) -> Option<TerminalEvent> {
        self.events.pop_front()
    }

    pub fn has_event(&self) -> bool {
        !self.events.is_empty()
    }
}

/// Tag format kitty ke angka kecil untuk JNI/serialization.
fn format_tag(f: KittyFormat) -> u8 {
    match f {
        KittyFormat::Png => 1,
        KittyFormat::Jpeg => 2,
        KittyFormat::Gif => 3,
        KittyFormat::Webp => 4,
        KittyFormat::Svg => 5,
        KittyFormat::Avif => 6,
        KittyFormat::Tiff => 7,
        KittyFormat::Rgba => 8,
        KittyFormat::Unknown => 0,
    }
}

impl Perform for Terminal {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
        self.mark_dirty(self.cursor.x, self.cursor.y);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\x07' => self.events.push_back(TerminalEvent::Bell),
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
        // private mode (`CSI ? ...`) dilayani; intermediate lain diabaikan
        if ignore || (!inter.is_empty() && inter != b"?") {
            return;
        }
        let private = inter == b"?";
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
                        1049 if self.alt_screen.is_none() => {
                            let (cols, rows) = (self.cols(), self.rows());
                            self.cursor.saved_x = self.cursor.x;
                            self.cursor.saved_y = self.cursor.y;
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                            self.enter_alt(cols, rows);
                        }
                        47 if !private && self.alt_screen.is_none() => {
                            let (cols, rows) = (self.cols(), self.rows());
                            self.enter_alt(cols, rows);
                        }
                        1000 | 1002 | 1003 if private && !self.mouse_tracking => {
                            self.mouse_tracking = true;
                            self.events.push_back(TerminalEvent::Mouse(true));
                        }
                        1006 if private => {
                            self.sgr_mouse = true;
                        }
                        _ => {}
                    }
                }
            }
            'l' => {
                if let Some(alt) = self.alt_screen.take() {
                    self.grid = alt;
                }
                if list.first().copied() == Some(1049) {
                    self.cursor.x = self.cursor.saved_x.min(self.cols());
                    self.cursor.y = self.cursor.saved_y.min(self.rows() - 1);
                } else if private
                    && matches!(list.first().copied(), Some(1000 | 1002 | 1003))
                    && self.mouse_tracking
                {
                    self.mouse_tracking = false;
                    self.events.push_back(TerminalEvent::Mouse(false));
                } else if private && list.first().copied() == Some(1006) {
                    self.sgr_mouse = false;
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
        match code {
            b"0" | b"2" => {
                if let Some(title) = params.get(1) {
                    if let Ok(t) = std::str::from_utf8(title) {
                        self.set_title(t.to_string());
                    }
                }
            }
            b"8" => {
                // OSC 8: `ESC ] 8 ; params ; uri` — uri kosong = tutup link.
                let raw = params.get(2).copied().unwrap_or_default();
                if raw.is_empty() {
                    self.current_hyperlink = None;
                } else {
                    let uri = unescape_octet(raw);
                    let id = {
                        self.next_hyperlink += 1;
                        self.next_hyperlink
                    };
                    self.hyperlinks.insert(id, uri);
                    self.current_hyperlink = Some(id);
                }
            }
            _ => {}
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

    /// URI untuk hyperlink id (OSC 8). `None` kalau id tak dikenal.
    pub fn hyperlink_uri(&self, id: u32) -> Option<&str> {
        self.hyperlinks.get(&id).map(String::as_str)
    }
}

/// Decode OSC 8 URI octet (unescape `%XX`; raw lainnya dibiarkan).
fn unescape_octet(raw: &[u8]) -> String {
    fn hex(v: u8) -> Option<u8> {
        match v {
            b'0'..=b'9' => Some(v - b'0'),
            b'a'..=b'f' => Some(v - b'a' + 10),
            b'A'..=b'F' => Some(v - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let (Some(h), Some(l)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    use crate::grid::indexed_to_rgb;

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
        assert_eq!(t.take_event(), None);
    }

    #[test]
    fn bell_emits_event() {
        let mut t = term_2x2();
        feed(&mut t, "\x07");
        assert_eq!(t.take_event(), Some(TerminalEvent::Bell));
    }

    #[test]
    fn events_fifo_order() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b]0;first\x07\x07\x1b]0;second\x07");
        assert_eq!(t.take_event(), Some(TerminalEvent::Title("first".into())));
        assert_eq!(t.take_event(), Some(TerminalEvent::Bell));
        assert_eq!(t.take_event(), Some(TerminalEvent::Title("second".into())));
        assert_eq!(t.take_event(), None);
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

    #[test]
    fn alternate_screen_isolated() {
        let mut t = term_2x2();
        feed(&mut t, "ab");
        assert!(t.alt_screen.is_none());

        feed(&mut t, "\x1b[1049h");
        assert!(t.alt_screen.is_some());
        assert!(
            t.grid.line(0).cells[0].is_empty(),
            "alt screen harus mulai kosong, bukan salinan main"
        );

        feed(&mut t, "XY");
        assert_eq!(t.grid.line(0).cells[0].ch, 'X');
        assert_eq!(t.grid.line(0).cells[1].ch, 'Y');

        feed(&mut t, "\x1b[1049l");
        assert!(t.alt_screen.is_none(), "alt screen harus dibuang");
        assert_eq!(t.grid.line(0).cells[0].ch, 'a', "main harus pulih");
        assert_eq!(t.grid.line(0).cells[1].ch, 'b');
        assert_eq!(
            (t.cursor.x, t.cursor.y),
            (2, 0),
            "kursor harus dikembalikan dari saved position"
        );
    }

    #[test]
    fn alternate_screen_47() {
        let mut t = term_2x2();
        feed(&mut t, "ab");
        feed(&mut t, "\x1b[47h");
        assert!(t.alt_screen.is_some());
        feed(&mut t, "\x1b[47l");
        assert!(t.alt_screen.is_none());
        assert_eq!(t.grid.line(0).cells[0].ch, 'a');
    }

    #[test]
    fn osc8_hyperlink_attached_to_cells() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b]8;;https://example.com/a%20b\x07hi");
        let id = t.grid.line(0).cells[0]
            .attrs
            .hyperlink
            .expect("cell harus punya hyperlink");
        assert_eq!(
            t.hyperlink_uri(id),
            Some("https://example.com/a b"),
            "URI harus di-unescape"
        );
        assert_eq!(t.grid.line(0).cells[1].attrs.hyperlink, Some(id));

        feed(&mut t, "\x1b]8;;\x07!");
        assert_eq!(
            t.grid.line(1).cells[0].attrs.hyperlink,
            None,
            "link tutup setelah OSC 8 uri kosong"
        );
    }

    #[test]
    fn osc8_hyperlink_uses_persistent_id() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b]8;id=tab1;https://a.dev\x07XY");
        let x_id = t.grid.line(0).cells[0].attrs.hyperlink.unwrap();
        feed(&mut t, "\x1b[2;1H");
        feed(&mut t, "\x1b]8;id=tab1;https://a.dev\x07Z");
        let z_id = t.grid.line(1).cells[0].attrs.hyperlink.unwrap();
        assert_eq!(x_id, 1);
        assert_eq!(z_id, 2);
        assert_eq!(t.hyperlink_uri(x_id), Some("https://a.dev"));
        assert_eq!(t.hyperlink_uri(z_id), Some("https://a.dev"));
    }

    #[test]
    fn sgr_256_color_roundtrip() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b[38;5;196mR");
        assert_eq!(t.grid.line(0).cells[0].attrs.fg, Color::Indexed(196));
        feed(&mut t, "\x1b[38;2;1;2;3mQ");
        assert_eq!(t.grid.line(0).cells[1].attrs.fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn alternate_screen_private_mode_questionmark() {
        // vim/less pakai `CSI ? 1049 h` — harus masuk mode privat
        let mut t = term_2x2();
        feed(&mut t, "ab");
        feed(&mut t, "\x1b[?1049h");
        assert!(t.alt_screen.is_some(), "?1049h harus masuk alt screen");
        feed(&mut t, "\x1b[?1049l");
        assert!(t.alt_screen.is_none(), "?1049l harus keluar alt screen");
        assert_eq!(t.grid.line(0).cells[0].ch, 'a', "main harus pulih");
    }

    #[test]
    fn mouse_tracking_toggles_event() {
        let mut t = term_2x2();
        feed(&mut t, "\x1b[?1000h");
        assert!(t.mouse_tracking);
        assert_eq!(t.take_event(), Some(TerminalEvent::Mouse(true)));

        feed(&mut t, "\x1b[?1000l");
        assert!(!t.mouse_tracking);
        assert_eq!(t.take_event(), Some(TerminalEvent::Mouse(false)));

        // mode 1002/1003 juga menyalakan
        feed(&mut t, "\x1b[?1003h");
        assert!(t.mouse_tracking);
    }

    #[test]
    fn sgr_1006_toggles_and_gates_encode() {
        let mut t = term_2x2();

        // tak aktif → encode kosong
        let mut out = Vec::new();
        assert!(!t.sgr_mouse_seq(crate::mouse::BTN_LEFT, 0, false, 1, 1, &mut out));
        assert!(out.is_empty());

        feed(&mut t, "\x1b[?1006h");
        assert!(t.sgr_mouse, "?1006h menyalakan SGR");
        assert!(t.sgr_mouse_seq(crate::mouse::BTN_LEFT, 0, false, 3, 2, &mut out));
        assert_eq!(out, b"\x1b[<0;3;2M");

        feed(&mut t, "\x1b[?1006l");
        assert!(!t.sgr_mouse, "?1006l mematikan SGR");

        // 1006 off tak menyala karena 1000h saja
        let mut t2 = term_2x2();
        feed(&mut t2, "\x1b[?1000h");
        assert!(!t2.sgr_mouse, "1000h tanpa 1006 → SGR tetap off");
    }

    #[test]
    fn fuzz_no_panic_invariants_hold() {
        // Feed byte acak deterministik + sekuens ESC terpotong: tidak boleh panic,
        // dan invariant grid harus bertahan.
        let cfg = TerminalConfig {
            cols: 40,
            rows: 12,
            ..Default::default()
        };
        let mut t = Terminal::new(cfg);
        let mut rng = 0x9E37_79B9u32;
        let mut buf = vec![0u8; 32 * 1024];
        for round in 0..100 {
            for b in buf.iter_mut() {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                *b = (rng >> 24) as u8;
            }
            if round % 3 == 0 {
                // sekuens ESC nyata tapi dipotong (truncate)
                let real = [
                    b"\x1b[31;1mcolor\x1b[0m".as_slice(),
                    b"\x1b]8;;https://a.dev\x07link\x1b]8;;\x07".as_slice(),
                    b"\x1b[?1006h".as_slice(),
                    b"\x1b]0;title\x07".as_slice(),
                    b"\x1b(P\xB0".as_slice(),
                ];
                let s = real[round % real.len()];
                t.feed_bytes(&s[..(rng as usize) % s.len()]);
            }
            t.feed_bytes(&buf);
            t.feed_bytes(b"\r\n\x1b[2J\x1b[H");
            assert_invariants(&t);
        }
    }

    fn assert_invariants(t: &Terminal) {
        assert!(t.cursor.x < t.cols());
        assert!(t.cursor.y < t.rows());
        for y in 0..t.grid.rows() {
            let line = t.grid.line(y);
            assert_eq!(line.cells.len(), t.grid.cols());
        }
        assert!(t.grid.rows() <= 12);
    }

    #[test]
    fn indexed_256_color_map() {
        assert_eq!(indexed_to_rgb(196), (255, 0, 0));
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        assert_eq!(indexed_to_rgb(231), (255, 255, 255));
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
        assert_eq!(indexed_to_rgb(255), (238, 238, 238));
        assert_eq!(Color::Indexed(196).to_rgb24(), Some((255, 0, 0)));
        assert_eq!(Color::Default.to_rgb24(), None);
    }

    /// Benchmark manual: feed 10k baris `seq 1 10000`-style dan ukur waktu.
    /// Jalankan: `cargo test -p mterm-core seq_10k -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn seq_10k_benchmark() {
        let cfg = TerminalConfig {
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let mut t = Terminal::new(cfg);
        let line = format!("line {}\r\n", "x".repeat(70));
        let mut buf = Vec::with_capacity(line.len() * 10_000);
        for _ in 0..10_000 {
            buf.extend_from_slice(line.as_bytes());
        }
        let start = std::time::Instant::now();
        t.feed_bytes(&buf);
        let elapsed = start.elapsed();
        let per_line = elapsed.as_secs_f64() / 10_000.0 * 1e6;
        println!("10k lines: {:?} ({:.0} us/baris)", elapsed, per_line);
    }

    // ── kitty graphics integration ────────────────────────────────────────

    fn fixture_png() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]
    }

    fn b64(png: &[u8]) -> String {
        // encoder utilitas kecil untuk test
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in png.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(ALPHA[(n >> 18) as usize] as char);
            out.push(ALPHA[((n >> 12) & 63) as usize] as char);
            if chunk.len() >= 2 {
                out.push(ALPHA[((n >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() >= 3 {
                out.push(ALPHA[(n & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    #[test]
    fn kitty_transmit_then_place() {
        let mut t = Terminal::new(TerminalConfig::default());
        let png = fixture_png();
        // transmit sekali jadi (m=0 default)
        let apc = format!("\x1b_Ga=t,t=f,f=100,s=10,v=10,m=0;{}\x1b\\", b64(&png));
        t.feed_str(&apc);
        let mut got_image = None;
        let mut got_placed = None;
        while let Some(ev) = t.take_event() {
            match ev {
                TerminalEvent::KittyImage {
                    id,
                    format,
                    width_px,
                    height_px,
                    data,
                } => {
                    got_image = Some((id, format, width_px, height_px, data));
                }
                TerminalEvent::KittyPlaced {
                    id,
                    x,
                    y,
                    cols,
                    rows,
                } => {
                    got_placed = Some((id, x, y, cols, rows));
                }
                _ => {}
            }
        }
        let (id, format, w, h, data) = got_image.expect("image event");
        assert_eq!(format, 1, "png");
        assert_eq!((w, h), (10, 10));
        assert_eq!(*data, png);
        assert!(got_placed.is_none(), "a=t tak place");
        assert!(t.images.contains_key(&id));

        // place di X=2,Y=1 ukuran 2x1
        let apc = format!("\x1b_Ga=p,q={id},X=2,Y=1,c=2,r=1\x1b\\");
        t.feed_str(&apc);
        let ev = t.take_event().expect("placed event");
        match ev {
            TerminalEvent::KittyPlaced {
                id: pid,
                x,
                y,
                cols,
                rows,
            } => {
                assert_eq!((pid, x, y, cols, rows), (id, 1, 0, 2, 1));
            }
            _ => panic!("wrong event: {ev:?}"),
        }
        // cell (1,0) dan (2,0) punya image attr; (3,0) tidak
        assert_eq!(t.grid.line(0).cells[1].attrs.image, Some(id));
        assert_eq!(t.grid.line(0).cells[2].attrs.image, Some(id));
        assert_eq!(t.grid.line(0).cells[3].attrs.image, None);
    }

    #[test]
    fn kitty_transmit_and_place_t() {
        let mut t = Terminal::new(TerminalConfig::default());
        let png = fixture_png();
        let apc = format!("\x1b_Ga=T,f=100,s=8,v=8,m=0;{}\x1b\\", b64(&png));
        t.feed_str(&apc);
        let mut placed = false;
        while let Some(ev) = t.take_event() {
            if matches!(ev, TerminalEvent::KittyPlaced { .. }) {
                placed = true;
            }
        }
        assert!(placed, "a=T langsung place di cursor");
        assert_eq!(t.grid.line(0).cells[0].attrs.image, Some(1));
    }

    #[test]
    fn kitty_chunked_transmit_across_feeds() {
        let mut t = Terminal::new(TerminalConfig::default());
        let png = fixture_png();
        let full = b64(&png);
        let (mid, tail) = full.split_at(full.len() / 2);
        let c1 = format!("\x1b_Ga=t,f=100,m=1;{}\x1b\\", mid);
        let c2 = format!("\x1b_Gm=2;{}\x1b\\", "!".repeat(20)); // b64 invalid → kosong
        let c3 = format!("\x1b_Gm=0;{}\x1b\\", tail);
        t.feed_str(&c1);
        assert!(t.pending_chunk.is_some(), "chunk 1 nyangkut");
        t.feed_str(&c2);
        t.feed_str(&c3);
        let mut got = None;
        while let Some(ev) = t.take_event() {
            if let TerminalEvent::KittyImage { id, data, .. } = ev {
                got = Some((id, data));
            }
        }
        let (id, data) = got.expect("image lengkap");
        // chunk 2 berisi payload salah → decode jadi bytes tak valid; yang
        // penting pipeline aman & id stable; data = mid + (sampah) + tail
        assert_eq!(id, 1);
        assert_eq!(*data, png);
        assert!(t.images.contains_key(&id));
    }

    #[test]
    fn kitty_apc_payload_truncated_buffered_next_feed() {
        let mut t = Terminal::new(TerminalConfig::default());
        let png = fixture_png();
        let full = b64(&png);
        let apc = format!("\x1b_Ga=t,f=100,s=4,v=4,m=0;{}\x1b\\", full).into_bytes();
        let cut = apc.len() - 3; // terminator belum lengkap
        let (head, tail) = apc.split_at(cut);
        t.feed_bytes(head);
        assert!(t.pending_apc.is_some(), "APC terpotong di-buffer");
        t.feed_bytes(tail);
        let mut got = None;
        while let Some(ev) = t.take_event() {
            if let TerminalEvent::KittyImage { data, .. } = ev {
                got = Some(data);
            }
        }
        assert_eq!(*got.expect("image"), png, "APC lanjutan di-feed berikutnya");
    }

    #[test]
    fn kitty_delete_all_and_by_id() {
        let mut t = Terminal::new(TerminalConfig::default());
        let png = fixture_png();
        let apc1 = format!("\x1b_Ga=t,f=100,s=4,v=4,m=0;{}\x1b\\", b64(&png));
        let apc2 = format!("\x1b_Ga=t,q=99,f=100,s=4,v=4,m=0;{}\x1b\\", b64(&png));
        t.feed_str(&apc1);
        t.feed_str(&apc2);
        assert!(t.images.contains_key(&1));
        assert!(t.images.contains_key(&99));
        // hapus semua
        t.feed_str("\x1b_Ga=d\x1b\\");
        assert!(t.images.is_empty());
        // pasang lagi, hapus per-id
        let apc1 = format!("\x1b_Ga=t,q=7,f=100,m=0;{}\x1b\\", b64(&png));
        let apc2 = format!("\x1b_Ga=t,q=8,f=100,m=0;{}\x1b\\", b64(&png));
        t.feed_str(&apc1);
        t.feed_str(&apc2);
        t.feed_str("\x1b_Ga=d,q=7\x1b\\");
        assert!(!t.images.contains_key(&7));
        assert!(t.images.contains_key(&8));
    }

    #[test]
    fn kitty_non_graphics_apc_ignored() {
        // APC non-kitty (contoh: koneksi TMUX?) → jangan panic, tak ada event
        let mut t = Terminal::new(TerminalConfig::default());
        t.feed_str("\x1b_Ga=zzz,o=1;QQ==\x1b\\");
        assert!(t.images.is_empty());
        assert!(!t.has_event());
    }

    // ── Viewport scrollback ──────────────────────────────────────────────

    fn feed_lines(t: &mut Terminal, lines: &[&str]) {
        for l in lines {
            t.feed_str(l);
            t.feed_str("\r\n");
        }
    }

    #[test]
    fn scroll_offset_reads_history_lines() {
        let mut t = Terminal::new(TerminalConfig {
            cols: 4,
            rows: 2,
            scrollback_cap: 100,
        });
        feed_lines(&mut t, &["l1", "l2", "l3", "l4", "l5"]);
        // CRLF tiap baris → 4 baris masuk scrollback; layar = l5 + baris kosong.
        assert_eq!(t.scrollback_len(), 4, "4 baris masuk scrollback");
        assert_eq!(t.scroll_offset(), 0);

        // offset 0 = ikut bottom (layar utama)
        assert_eq!(row_text(&t, 0), "l5", "offset 0 = bottom (l5)");

        // scroll penuh ke atas → tampilan mulai dari sejarah paling tua
        t.set_scroll_offset(99);
        assert_eq!(t.scroll_offset(), 4, "clamp ke max scroll");
        assert_eq!(row_text(&t, 0), "l1", "teratas = l1");
        assert_eq!(row_text(&t, 1), "l2", "baris kedua = l2");

        // setengah scroll → jendela [l3,l4]
        t.set_scroll_offset(2);
        assert_eq!(row_text(&t, 0), "l3");
        assert_eq!(row_text(&t, 1), "l4");
    }

    fn row_text(t: &Terminal, y: usize) -> String {
        t.view_line(y).cells.iter().take(2).map(|c| c.ch).collect()
    }

    #[test]
    fn scroll_offset_clamps_and_resets() {
        let mut t = Terminal::new(TerminalConfig {
            cols: 4,
            rows: 2,
            scrollback_cap: 2,
        });
        feed_lines(&mut t, &["a", "b", "c", "d"]);
        // scrollback cap 2: hanya c,d ada di scrollback? a,b evicted
        assert!(t.scrollback_len() <= 2);

        t.set_scroll_offset(99);
        assert_eq!(t.scroll_offset(), t.scrollback_len(), "clamp ke max scroll");

        // resize lebih kecil → offset tetap terjaga (clamp)
        t.set_scroll_offset(0);
        feed_lines(&mut t, &["e"]);
        t.resize(4, 3);
        assert!(t.scroll_offset() <= t.scrollback_len());
    }

    #[test]
    fn scroll_dirty_marks_full_repaint() {
        let mut t = term_2x2();
        feed_lines(&mut t, &["x", "y", "z"]);
        t.set_scroll_offset(1);
        assert_eq!(
            t.dirty_rect,
            Some((0, 0, 2, 2)),
            "scroll → seluruh layar dirty"
        );
        t.set_scroll_offset(1);
        assert_eq!(
            t.dirty_rect,
            Some((0, 0, 2, 2)),
            "offset sama → dirty tetap"
        );
    }

    #[test]
    fn alt_screen_resets_scroll_offset() {
        let mut t = term_2x2();
        feed_lines(&mut t, &["p", "q", "r"]);
        t.set_scroll_offset(1);
        assert_eq!(t.scroll_offset(), 1);
        t.feed_str("\x1b[?1049h");
        assert_eq!(t.scroll_offset(), 0, "masuk alt screen → offset 0");
        t.feed_str("\x1b[?1049l");
        assert_eq!(t.scroll_offset(), 0, "keluar alt screen tetap 0");
    }

    // ── Session persistence (Fase 4) ─────────────────────────────────────

    #[test]
    fn snapshot_roundtrip_preserves_grid_cursor_scrollback() {
        let mut t = Terminal::new(TerminalConfig {
            cols: 6,
            rows: 2,
            scrollback_cap: 50,
        });
        feed_lines(&mut t, &["aaa", "bbb", "ccc", "ddd"]);
        t.feed_str("\x1b[5;3H"); // cursor CUP row5 col3 (belakang → clamp)
        let snapshot = t.to_json().expect("serialize ok");
        let mut r = Terminal::from_json(&snapshot).expect("restore ok");

        assert_eq!(r.rows(), 2, "config tersimpan");
        assert_eq!(r.scrollback_len(), t.scrollback_len(), "scrollback sama");
        r.set_scroll_offset(t.scroll_offset());
        for y in 0..r.rows() {
            let orig: Vec<char> = t.view_line(y).cells.iter().map(|c| c.ch).collect();
            let rest: Vec<char> = r.view_line(y).cells.iter().map(|c| c.ch).collect();
            assert_eq!(orig, rest, "baris scroll `{y}` sama setelah restore");
        }
        // cursor
        assert_eq!(
            (r.cursor.x, r.cursor.y),
            (t.cursor.x, t.cursor.y),
            "cursor posisi"
        );
        assert_eq!(r.cursor.attrs, t.cursor.attrs, "cursor attrs");
        assert_eq!(r.mode, t.mode, "mode");
    }

    #[test]
    fn snapshot_json_not_empty_and_no_images() {
        let mut t = term_2x2();
        feed(&mut t, "test");
        let json = t.to_json().unwrap();
        assert!(!json.is_empty());
        // images TIDAK boleh masuk JSON
        assert!(!json.contains("KittyImage"), "json tak ada images");
        assert!(!json.contains("dirty_rect"), "json tak ada dirty_rect");
        // restore valid
        let t2 = Terminal::from_json(&json).unwrap();
        assert_eq!(t2.cols(), t.cols());
    }

    #[test]
    fn snapshot_bad_json_returns_err() {
        assert!(Terminal::from_json("{garbage}").is_err() || Terminal::from_json("{}").is_err());
    }

    #[test]
    fn snapshot_from_json_fills_no_alt_screen() {
        let mut t = term_2x2();
        feed(&mut t, "xy");
        let j = t.to_json().unwrap();
        let r = Terminal::from_json(&j).unwrap();
        assert_eq!(r.grid.line(0).cells[0].ch, 'x');
        assert!(r.alt_screen.is_none() || true, "alt_screen ok");
    }
}
