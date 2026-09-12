//! Renderer kecil markdown → ANSI (tanpa dep) buat output agent di CLI.
//! Subset: heading, bold/italic/inline-code, fenced code, list, blockquote,
//! horizontal rule, link `[t](u)` (URL dibuang, teks tetap). Tak sempurna —
//! cukup terbaca di terminal.

const RESET: &str = "\x1b[0m";
const STYLES: [&str; 6] = [
    "\x1b[38;5;148m", // inline code (hijau)
    "\x1b[1m",        // bold
    "\x1b[3m",        // italic
    "\x1b[1;32m",     // h1
    "\x1b[1;33m",     // h2
    "\x1b[1;36m",     // h3
];
const H1: usize = 3;
const H2: usize = 4;
const H3: usize = 5;

/// Konversi markdown → ANSI. Kalau `color: false`, teks polos (tanpa escape).
pub fn markdown_to_ansi(md: &str, color: bool) -> String {
    if !color {
        return md.to_string();
    }
    let mut out = String::with_capacity(md.len() + md.len() / 8);
    let mut para: Vec<&str> = Vec::new();
    let mut fence: Option<String> = None;

    for raw in md.lines() {
        let line = raw.trim_end();
        let t = line.trim();

        if t.starts_with("```") {
            flush(&mut out, std::mem::take(&mut para));
            if fence.is_some() {
                fence = None;
            } else {
                fence = Some(t.trim_start()[3..].trim().to_string());
            }
            continue;
        }
        if fence.is_some() {
            fence_line(&mut out, t);
            continue;
        }
        if block(&mut out, t) {
            continue;
        }
        if !t.is_empty() {
            para.push(line);
        }
    }
    flush(&mut out, para);
    if fence.is_some() {
        out.push('\n');
    }
    out
}

fn flush(out: &mut String, para: Vec<&str>) {
    let text = para.join("\n").trim().to_string();
    if !text.is_empty() {
        out.push_str(&inline(&text));
        out.push_str("\n\n");
    }
}

fn fence_line(out: &mut String, t: &str) {
    out.push_str(STYLES[0]);
    out.push_str(t);
    out.push_str(RESET);
    out.push('\n');
}

fn block(out: &mut String, t: &str) -> bool {
    if let Some(body) = t.strip_prefix("###") {
        heading(out, body.trim(), H3);
        return true;
    }
    if let Some(body) = t.strip_prefix("##") {
        heading(out, body.trim(), H2);
        return true;
    }
    if let Some(body) = t.strip_prefix('#') {
        heading(out, body.trim(), H1);
        return true;
    }
    if let Some(body) = t.strip_prefix('-') {
        out.push_str(&format!("  • {}\n", inline(body.trim())));
        return true;
    }
    if let Some(body) = t.strip_prefix('*') {
        out.push_str(&format!("  • {}\n", inline(body.trim())));
        return true;
    }
    if let Some(body) = t.strip_prefix('>') {
        out.push_str(&format!("  │ {}{RESET}\n", inline(body.trim())));
        return true;
    }
    if t.trim_end() == "---" || t.trim_end() == "***" {
        out.push_str(&format!("{}\n", "-".repeat(48)));
        return true;
    }
    if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        if let Some(body) = t
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .strip_prefix('.')
            .and_then(|b| b.strip_prefix(' '))
        {
            out.push_str(&format!("  {}\n", inline(body.trim())));
            return true;
        }
    }
    false
}

fn heading(out: &mut String, body: &str, style: usize) {
    out.push_str(STYLES[style]);
    out.push_str(&inline(body));
    out.push_str(RESET);
    out.push('\n');
}

/// Inline: `code`, **bold**, *italic*, [teks](url). Pakai `Vec<char>` biar
/// index aman untuk UTF-8.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(rel) = chars[i + 1..].iter().position(|&x| x == '`') {
                let code: String = chars[i + 1..i + 1 + rel].iter().collect();
                out.push_str(STYLES[0]);
                out.push_str(&code);
                out.push_str(RESET);
                i = i + 1 + rel + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(rel) = chars[i + 2..].windows(2).position(|w| w == ['*', '*']) {
                let body: String = chars[i + 2..i + 2 + rel].iter().collect();
                out.push_str(STYLES[1]);
                out.push_str(&body);
                out.push_str(RESET);
                i = i + 2 + rel + 2;
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '*' {
            if let Some(rel) = chars[i + 1..].iter().position(|&x| x == '*') {
                let body: String = chars[i + 1..i + 1 + rel].iter().collect();
                out.push_str(STYLES[2]);
                out.push_str(&body);
                out.push_str(RESET);
                i = i + 1 + rel + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '[' {
            if let Some(rel) = chars[i + 1..].iter().position(|&x| x == ']') {
                let text: String = chars[i + 1..i + 1 + rel].iter().collect();
                out.push_str(&text);
                i = i + 1 + rel;
                if i + 1 < chars.len() && chars[i + 1] == '(' {
                    if let Some(pend) = chars[i + 2..].iter().position(|&x| x == ')') {
                        i = i + 2 + pend + 1;
                        continue;
                    }
                }
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_when_no_color() {
        let md = "# Judul\n\n**tebal** dan `kode` dan [link](https://x.test)\n";
        let out = markdown_to_ansi(md, false);
        assert!(out.contains("**tebal**"), "tanpa style harus polos");
        assert!(!out.contains('\x1b'), "no escape");
    }

    #[test]
    fn headings_lists_fence_rendered() {
        let md = "# H1\n## H2\n- item\n```rs\nlet x = 1;\n```\n> kutipan\n";
        let out = markdown_to_ansi(md, true);
        assert!(out.contains("\x1b[1;32m"), "H1 style");
        assert!(out.contains("\x1b[1;33m"), "H2 style");
        assert!(out.contains("• item"));
        assert!(out.contains("let x = 1;"));
        assert!(out.contains("│ kutipan"), "blockquote");
    }

    #[test]
    fn inline_bold_code_link() {
        let out = inline("**bold** `x` [a](https://u.test) *miring*");
        assert!(out.contains("\x1b[1m") && out.contains("bold"));
        assert!(out.contains("x") && out.contains("\x1b[38;5;148m"));
        assert!(!out.contains("https://u.test"), "url link dibuang");
        assert!(out.contains("a"), "teks link dipertahankan");
        // absorb ANSI (yang berisi `[`) lalu pastikan tak ada bracket markdown bocor
        let plain = out
            .replace(STYLES[0], "")
            .replace(STYLES[1], "")
            .replace(STYLES[2], "")
            .replace(STYLES[3], "")
            .replace(STYLES[4], "")
            .replace(STYLES[5], "")
            .replace(RESET, "");
        assert!(!plain.contains('['), "kurung siku tak bocor: {plain:?}");
        assert!(!plain.contains('('), "kurung url tak bocor: {plain:?}");
    }

    #[test]
    fn table_passthrough_no_heading() {
        let out = markdown_to_ansi("| a | b |\n|---|---|\n| 1 | 2 |\n", true);
        assert!(!out.contains("\x1b[1;32m"), "baris table bukan heading");
    }

    #[test]
    fn text_after_fence_is_paragraph_not_code() {
        let md = "```rs\nlet x = 1;\n```\nparagraf setelah fence\n";
        let out = markdown_to_ansi(md, true);
        assert!(out.contains("let x = 1;"), "isi fence");
        let plain = out.replace("\x1b[38;5;245m", "").replace(RESET, "");
        // paragraf harus lewat inline (style code 245 tidak menyentuh "paragraf")
        let after_para = out[out.find("paragraf setelah").unwrap()..].to_string();
        assert!(
            !after_para.starts_with("\x1b[38;5;245m"),
            "paragraf tak boleh jadi code-fence: {plain:?}"
        );
    }

    #[test]
    fn unicode_inline_utf8() {
        let out = inline("halo **dunia 🌍** dan `kode`");
        assert!(out.contains("dunia 🌍"), "utf-8 badan bold utuh");
        assert!(out.contains("kode"));
    }
}