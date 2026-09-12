//! Encoding mouse input ke xterm SGR mode (CSI 1006).
//!
//! Chrome (Android touch) mengirim angka button/koor + modif; engine ini
//! mengubahnya menjadi escape `CSI < Cb ; Cx ; Cy M/m`. Kalau mode SGR
//! belum aktif, `encode` mengembalikan `None`.

/// Bit modif xterm (di-OR ke button code):
pub const MOD_SHIFT: u8 = 4;
pub const MOD_ALT: u8 = 8;
pub const MOD_CTRL: u8 = 16;
/// Bit gerakan (motion tracking di 1002/1003 → kode naik 32).
pub const MOTION: u8 = 32;

/// Kode tombol dasar: 0=left, 1=middle, 2=right.
pub const BTN_LEFT: u8 = 0;
pub const BTN_MIDDLE: u8 = 1;
pub const BTN_RIGHT: u8 = 2;
/// Rod (wheel): 64=atas, 65=bawah. Tidak punya event "release".
pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;

/// Encode satu event mouse menjadi byte SGR (tanpa memeriksa mode).
///
/// - `code`: `BTN_*` atau `WHEEL_*`; `release: true` menaikkan 3 untuk tombol
///   0..=2 (sesuai protokol SGR: M=motion/press, m=release).
/// - `mods`: bit `MOD_*` (boleh termasuk `MOTION` dari caller).
/// - `x`/`y`: kolom/baris 1-based (koor 0 → dijamin ≥1).
///
/// Contoh: klik kiri di kolom 3 baris 2 → `\x1b[<0;3;2M`.
pub fn encode(code: u8, mods: u8, release: bool, x: usize, y: usize, out: &mut Vec<u8>) {
    let mut b = code;
    if release && b < 64 {
        b += 3;
    }
    b += mods;
    let x = x.max(1);
    let y = y.max(1);
    let term = if release { b'm' } else { b'M' };
    out.extend_from_slice(b"\x1b[<");
    append_num(out, b as usize);
    out.push(b';');
    append_num(out, x);
    out.push(b';');
    append_num(out, y);
    out.push(term);
}

fn append_num(out: &mut Vec<u8>, n: usize) {
    if n == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = n;
    while n > 0 {
        i -= 1;
        buf[i] = (n % 10) as u8 + b'0';
        n /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(code: u8, mods: u8, release: bool, x: usize, y: usize) -> String {
        let mut v = Vec::new();
        encode(code, mods, release, x, y, &mut v);
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn press_left() {
        assert_eq!(enc(BTN_LEFT, 0, false, 3, 2), "\x1b[<0;3;2M");
    }

    #[test]
    fn release_left_adds_three() {
        assert_eq!(enc(BTN_LEFT, 0, true, 3, 2), "\x1b[<3;3;2m");
    }

    #[test]
    fn right_click_has_code_two() {
        assert_eq!(enc(BTN_RIGHT, 0, false, 1, 1), "\x1b[<2;1;1M");
    }

    #[test]
    fn modifiers_are_orred() {
        // xterm modif bits: Shift=4, Alt=8, Ctrl=16
        assert_eq!(enc(BTN_LEFT, MOD_CTRL, false, 5, 5), "\x1b[<16;5;5M");
        assert_eq!(enc(BTN_MIDDLE, MOD_SHIFT, false, 5, 5), "\x1b[<5;5;5M");
    }

    #[test]
    fn motion_uses_bit_32() {
        assert_eq!(enc(BTN_LEFT, MOTION, false, 7, 9), "\x1b[<32;7;9M");
    }

    #[test]
    fn wheel_down() {
        // roda tanpa modif → kode 65 (tanpa +32 basis)
        assert_eq!(enc(WHEEL_DOWN, 0, false, 4, 4), "\x1b[<65;4;4M");
    }

    #[test]
    fn zero_coords_clamped() {
        assert_eq!(enc(BTN_LEFT, 0, false, 0, 0), "\x1b[<0;1;1M");
    }

    #[test]
    fn large_coords_ok() {
        assert_eq!(enc(BTN_LEFT, 0, false, 200, 50), "\x1b[<0;200;50M");
    }
}
