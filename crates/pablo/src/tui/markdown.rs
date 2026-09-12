//! Small, bounded terminal Markdown presentation; only our own SGR escapes are emitted.
pub fn columns(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036f | 0x200d | 0xfe00..=0xfe0f | 0x1f3fb..=0x1f3ff => 0,
        0x1100..=0x115f
        | 0x2329..=0x232a
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff01..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1faff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}
fn sgr(style: u8) -> String {
    let mut value = String::from("\x1b[0");
    for (bit, code) in [
        (1, ";1"),
        (2, ";3"),
        (4, ";36"),
        (8, ";4"),
        (16, ";2"),
        (32, ";9"),
    ] {
        if style & bit != 0 {
            value.push_str(code);
        }
    }
    value.push('m');
    value
}
struct Line {
    rows: Vec<String>,
    text: String,
    width: usize,
    used: usize,
    style: u8,
}
impl Line {
    fn new(width: usize) -> Self {
        Self {
            rows: Vec::new(),
            text: String::new(),
            width: width.max(1),
            used: 0,
            style: 0,
        }
    }
    fn style(&mut self, style: u8) {
        if self.style != style {
            self.style = style;
            self.text.push_str(&sgr(style));
        }
    }
    fn write(&mut self, text: &str) {
        for c in text.chars() {
            let c = if c.is_control() { '�' } else { c };
            let cost = columns(c);
            if self.used + cost > self.width && self.used > 0 {
                self.text.push_str("\x1b[0m");
                self.rows.push(std::mem::take(&mut self.text));
                self.used = 0;
                if self.style != 0 {
                    self.text.push_str(&sgr(self.style));
                }
            }
            if cost <= self.width {
                self.text.push(c);
                self.used += cost;
            }
        }
    }
    fn finish(mut self) -> Vec<String> {
        if self.style != 0 {
            self.text.push_str("\x1b[0m");
        }
        self.rows.push(self.text);
        self.rows
    }
}
fn inline(out: &mut Line, mut text: &str, base: u8) {
    let mut flags = 0;
    let mut style = base;
    out.style(style);
    while !text.is_empty() {
        if let Some(rest) = text.strip_prefix('`') {
            flags ^= 4;
            style = base | flags;
            out.style(style);
            text = rest;
            continue;
        }
        if style & 4 == 0 {
            let token = if text.starts_with("**") || text.starts_with("__") {
                Some((2, 1))
            } else if text.starts_with("~~") {
                Some((2, 32))
            } else if text.starts_with('*') {
                Some((1, 2))
            } else {
                None
            };
            if let Some((bytes, bit)) = token {
                flags ^= bit;
                style = base | flags;
                out.style(style);
                text = &text[bytes..];
                continue;
            }
            // Bounded lookahead keeps pathological unmatched brackets linear in input size.
            if let Some(rest) = text.strip_prefix('[') {
                let mut end = rest.len().min(512);
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                let window = &rest[..end];
                if let Some(split) = window.find("](")
                    && let Some(close) = window[split + 2..].find(')')
                {
                    out.style(style | 8);
                    out.write(&window[..split]);
                    out.style(style);
                    out.write(" (");
                    out.write(&window[split + 2..split + 2 + close]);
                    out.write(")");
                    text = &rest[split + 3 + close..];
                    continue;
                }
            }
        }
        let c = text.chars().next().unwrap();
        out.write(&text[..c.len_utf8()]);
        text = &text[c.len_utf8()..];
    }
}
pub fn render(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut fence: Option<&str> = None;
    for source in text.split('\n') {
        let trimmed = source.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            if fence == Some(marker) {
                fence = None;
                rows.push(String::new());
                continue;
            }
            if fence.is_none() {
                fence = Some(marker);
                let mut out = Line::new(width);
                out.style(16);
                out.write(if trimmed[3..].trim().is_empty() {
                    "code"
                } else {
                    trimmed[3..].trim()
                });
                rows.extend(out.finish());
                continue;
            }
        }
        let mut out = Line::new(width);
        if fence.is_some() {
            out.style(4);
            out.write("  ");
            out.write(source);
        } else {
            let heading = source.bytes().take_while(|b| *b == b'#').count();
            if (1..=6).contains(&heading) && source.as_bytes().get(heading) == Some(&b' ') {
                inline(&mut out, &source[heading + 1..], 1);
            } else if matches!(trimmed, "---" | "***" | "___") {
                out.style(16);
                out.write(&"─".repeat(width));
            } else if let Some(body) = trimmed.strip_prefix("> ") {
                out.style(16);
                out.write("│ ");
                inline(&mut out, body, 16);
            } else if let Some(body) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .or_else(|| trimmed.strip_prefix("+ "))
            {
                out.write(&source[..source.len() - trimmed.len()]);
                out.write("• ");
                inline(&mut out, body, 0);
            } else {
                inline(&mut out, source, 0);
            }
        }
        rows.extend(out.finish());
    }
    rows
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_styles_blocks_and_wraps_without_counting_escapes_as_columns() {
        let rows = render(
            "# Title\n**bold** and `code`\n- item\n> quote\n```rust\nlet x = **raw**;\n```\n[site](https://example.test)",
            80,
        );
        let output = rows.join("\n");
        assert!(output.contains("\x1b[0;1mTitle"));
        assert!(output.contains("• item") && output.contains("│ quote"));
        assert!(output.contains("let x = **raw**;") && !output.contains("```"));
        assert!(output.contains("https://example.test") && !output.contains("[site]("));
        assert_eq!(
            render("**123456**", 3),
            ["\x1b[0;1m123\x1b[0m", "\x1b[0;1m456\x1b[0m"]
        );
    }
}
