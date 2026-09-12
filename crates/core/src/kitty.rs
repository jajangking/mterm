//! Kitty graphics protocol — subset minimal: transmit, transmit+place, place,
//! delete, query-ignore. Parsing APC `ESC _ G ... ESC \`.
//!
//! Supported:
//! - `a=t` (transmit file, chunked via `m=1..m=0`, payload base64)
//! - `a=T` (transmit + place di position)
//! - `a=p` (place image yang sudah ada, ref `q=`)
//! - `a=d` (delete: `q=<id>` atau `i=-1`/kosong = hapus semua)
//! - `a=q` (query → diabaikan, emulator lain urus)
//!
//! Tak didukung: kompresi `o=` (zlib/brotli), `a=f` frame, animasi GIF,
//! four-step bitcopy `q=` — data ditag dan diteruskan apa adanya.

use std::sync::Arc;

/// Format gambar yang dikirim via protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Avif,
    Tiff,
    Rgba,
    #[default]
    Unknown,
}

impl KittyFormat {
    /// Kode `f=...` dari header APC (angka kitty klasik + string modern).
    fn from_header(s: &str) -> KittyFormat {
        match s {
            "png" | "100" => KittyFormat::Png,
            "jpeg" | "jpg" | "101" => KittyFormat::Jpeg,
            "gif" | "102" => KittyFormat::Gif,
            "webp" | "103" => KittyFormat::Webp,
            "svg" | "104" => KittyFormat::Svg,
            "avif" | "105" => KittyFormat::Avif,
            "tiff" | "106" => KittyFormat::Tiff,
            "r" | "rgba" | "q" => KittyFormat::Rgba,
            _ => KittyFormat::Unknown,
        }
    }
}

/// Action yang diminta oleh APC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyAction {
    Transmit,
    TransmitAndPlace,
    Place,
    Delete,
    Query,
    #[default]
    Ignore,
}

/// Satu APC `kitty` yang sudah diparse.
#[derive(Debug, Default, Clone)]
pub struct KittyCommand {
    pub action: KittyAction,
    /// `f=` format (untuk transmit).
    pub format: KittyFormat,
    /// `q=` image id (autoincrement kalau None).
    pub image_id: Option<u32>,
    /// `i=` image number (delete-all = -1).
    pub image_no: Option<i32>,
    /// `m=` more: 0 akhir, 1 lanjut, -1 direct.
    pub more: i8,
    /// `X=`/`Y=` posisi place (1-based, optional).
    pub x: Option<u32>,
    pub y: Option<u32>,
    /// `c=`/`r=` sel.
    pub cols: u32,
    pub rows: u32,
    /// `s=`/`v=` ukuran px (metadata).
    pub width_px: u32,
    pub height_px: u32,
    /// `A=` minta respons (1).
    pub want_reply: bool,
    /// Payload setelah `;` (base64 untuk transmit file).
    pub payload: Vec<u8>,
}

/// Parse isi APC (antara `ESC _` dan `ESC \`).
pub fn parse_apc(content: &[u8]) -> KittyCommand {
    let text = String::from_utf8_lossy(content);
    // pisah header `k=v,...` dan payload setelah `;`
    let (header, payload) = match text.split_once(';') {
        Some((h, p)) => (h, p),
        None => (text.as_ref(), ""),
    };
    let mut cmd = KittyCommand::default();
    let mut action = None;

    for kv in header.split(',') {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        match k {
            "a" => {
                action = Some(match v {
                    "t" => KittyAction::Transmit,
                    "T" => KittyAction::TransmitAndPlace,
                    "p" => KittyAction::Place,
                    "d" => KittyAction::Delete,
                    "q" => KittyAction::Query,
                    unsupported if !unsupported.is_empty() => KittyAction::Ignore,
                    _ => KittyAction::Ignore,
                })
            }
            "f" => cmd.format = KittyFormat::from_header(v),
            "q" => cmd.image_id = v.parse::<u32>().ok(),
            "i" => cmd.image_no = v.parse::<i32>().ok(),
            "m" => {
                cmd.more = v.parse::<i8>().unwrap_or(0);
            }
            "X" => cmd.x = v.parse::<u32>().ok(),
            "Y" => cmd.y = v.parse::<u32>().ok(),
            "c" => {
                if let Ok(n) = v.parse::<u32>() {
                    cmd.cols = n.max(1);
                }
            }
            "r" => {
                if let Ok(n) = v.parse::<u32>() {
                    cmd.rows = n.max(1);
                }
            }
            "s" => cmd.width_px = v.parse::<u32>().unwrap_or(0),
            "v" => cmd.height_px = v.parse::<u32>().unwrap_or(0),
            "A" => cmd.want_reply = v == "1",
            _ => {}
        }
    }

    if payload.is_empty() {
        // a=p / a=d / a=q tak punya payload; identifikasi dari header saja
        cmd.action = action.unwrap_or(KittyAction::Ignore);
        return cmd;
    }

    // payload bisa pakai `o=`/`s=`/`v=` jumpa; decode base64 kalau bukan direct
    let trimmed = payload.trim_end_matches(['\r', '\n']);
    match action {
        Some(KittyAction::TransmitAndPlace) => {
            cmd.action = KittyAction::TransmitAndPlace;
            if cmd.more == -1 {
                cmd.payload = trimmed.as_bytes().to_vec();
            } else {
                cmd.payload = b64_decode(trimmed.as_bytes()).unwrap_or_default();
            }
        }
        Some(KittyAction::Transmit) => {
            cmd.action = KittyAction::Transmit;
            if cmd.more == -1 {
                cmd.payload = trimmed.as_bytes().to_vec();
            } else {
                cmd.payload = b64_decode(trimmed.as_bytes()).unwrap_or_default();
            }
        }
        None if !payload.is_empty() => {
            // chunk lanjutan (tidak ada `a=` di header) → transmisi
            cmd.action = KittyAction::Transmit;
            cmd.payload = b64_decode(trimmed.as_bytes()).unwrap_or_default();
        }
        _ => cmd.action = action.unwrap_or(KittyAction::Ignore),
    }
    cmd
}

/// Setengah-transmitted image (buat chunk `m=1..m=0`).
#[derive(Debug, Default, Clone)]
pub struct PendingChunk {
    pub bytes: Vec<u8>,
    pub format: KittyFormat,
    pub width_px: u32,
    pub height_px: u32,
    pub image_id: u32,
}

/// Gambar utuh di registry terminal.
#[derive(Debug, Clone)]
pub struct KittyImage {
    pub format: KittyFormat,
    pub width_px: u32,
    pub height_px: u32,
    pub data: Arc<Vec<u8>>,
}

/// Hasil transmit yang sudah lengkap (internal pemindahan ke registry).
#[derive(Debug)]
pub struct Completed {
    pub id: u32,
    pub format: KittyFormat,
    pub width_px: u32,
    pub height_px: u32,
    pub bytes: Vec<u8>,
}

/// Decode base64 (padding opsional). Cukup untuk payload gambar.
pub fn b64_decode(data: &[u8]) -> Option<Vec<u8>> {
    const T: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0;
        while i < 26 {
            t[b'A' as usize + i] = i as i8;
            t[b'a' as usize + i] = i as i8 + 26;
            i += 1;
        }
        let mut i = 0;
        while i < 10 {
            t[b'0' as usize + i] = i as i8 + 52;
            i += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };

    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &c in data {
        if c == b'=' {
            break;
        }
        let v = T[c as usize];
        if v < 0 {
            continue; // newline/spasi di-ignore
        }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_roundtrip() {
        for (raw, enc) in [
            (b"hello".as_slice(), "aGVsbG8=".as_bytes()),
            (b"a".as_slice(), "YQ==".as_bytes()),
            (&[], &[] as &[u8]),
            (
                b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".as_slice(),
                b"iVBORw0KGgoAAAANSUhEUg==",
            ),
        ] {
            assert_eq!(b64_decode(enc).unwrap(), raw);
        }
    }

    #[test]
    fn parse_transmit_t() {
        let apc = "a=t,t=f,f=100,s=64,v=64,m=1;aGVsbG8=";
        let cmd = parse_apc(apc.as_bytes());
        assert_eq!(cmd.action, KittyAction::Transmit);
        assert_eq!(cmd.format, KittyFormat::Png);
        assert_eq!(cmd.width_px, 64);
        assert_eq!(cmd.height_px, 64);
        assert_eq!(cmd.more, 1);
        assert_eq!(cmd.payload, b"hello");
    }

    #[test]
    fn parse_transmit_and_place_no_payload() {
        // tanpa `;` → Place / Delete / Query tak punya payload
        let cmd = parse_apc(b"a=p,q=7,c=2,r=2");
        assert_eq!(cmd.action, KittyAction::Place);
        assert_eq!(cmd.image_id, Some(7));
        assert_eq!(cmd.cols, 2);
        assert_eq!(cmd.rows, 2);
    }

    #[test]
    fn parse_delete_without_payload() {
        let cmd = parse_apc(b"a=d,i=-1");
        assert_eq!(cmd.action, KittyAction::Delete);
        assert_eq!(cmd.image_no, Some(-1));
    }

    #[test]
    fn parse_png_format_number() {
        let cmd = parse_apc(b"a=t,t=f,f=100;eA==");
        assert_eq!(cmd.format, KittyFormat::Png);
        assert_eq!(cmd.payload, b"x");
    }

    #[test]
    fn parse_unknown_action_ignored() {
        let cmd = parse_apc(b"a=zzz,o=1");
        assert_eq!(cmd.action, KittyAction::Ignore);
    }
}
