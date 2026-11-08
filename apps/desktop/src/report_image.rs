//! Summary image export: real PNG pixels, with wrapping for long tasks and descriptions.
use anyhow::{Context, Result, ensure};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn wrap(text: &str, limit: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        let mut count = 0;
        for word in paragraph.split_whitespace() {
            let length = word.chars().count();
            if count > 0 && count + length + 1 > limit {
                lines.push(std::mem::take(&mut line));
                count = 0;
            }
            if count > 0 { line.push(' '); count += 1; }
            for ch in word.chars() {
                if count == limit {
                    lines.push(std::mem::take(&mut line));
                    count = 0;
                }
                line.push(ch);
                count += 1;
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    if lines.is_empty() { lines.push(String::new()); }
    lines
}

pub fn png(heading: &str, groups: &[(String, Vec<String>)]) -> Result<Vec<u8>> {
    let mut lines = Vec::new();
    for line in wrap(heading, 55) { lines.push((line, 24, true)); }
    for (title, items) in groups {
        if !title.is_empty() {
            lines.push((String::new(), 16, false));
            for line in wrap(title, 70) { lines.push((line, 18, true)); }
        }
        for item in items {
            for line in wrap(item, 82) { lines.push((line, 16, false)); }
        }
    }
    let height = 72 + lines.len() as u32 * 30;
    ensure!(height <= 32_000, "Report is too long for one image. Save it as PDF instead.");
    let mut svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{height}"><rect width="100%" height="100%" fill="white"/><g fill="#24262b" font-family="Segoe UI,Arial,sans-serif">"##);
    for (i, (line, size, bold)) in lines.iter().enumerate() {
        svg.push_str(&format!(r#"<text x="40" y="{}" font-size="{size}" font-weight="{}">{}</text>"#, 42 + i * 30, if *bold { "bold" } else { "normal" }, escape(line)));
    }
    svg.push_str("</g></svg>");
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(&svg, &options).context("cannot render the report")?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(1000, height).context("cannot allocate the report image")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().context("cannot encode the report image")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_is_png_and_wraps_multilingual_content() {
        let text = "Отчёт <edited> & tasks ".repeat(20);
        assert!(wrap(&text, 82).iter().all(|line| line.chars().count() <= 82));
        let bytes = png("Отчёт", &[("Completed".into(), vec![text])]).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = resvg::tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!(decoded.width(), 1000);
        assert!(decoded.pixels().iter().any(|pixel| pixel.red() < 150));
    }
}
