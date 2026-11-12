//! Summary image export: real PNG pixels, with wrapping for long tasks and descriptions.
use anyhow::{Context, Result, ensure};

const REPORT_WIDTH: u32 = 1000;
const MAX_REPORT_PIXELS: u64 = 32_000_000;

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
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    render_png(heading, groups, options)
}

fn render_png(
    heading: &str,
    groups: &[(String, Vec<String>)],
    mut options: resvg::usvg::Options<'_>,
) -> Result<Vec<u8>> {
    // System fonts vary across platforms. Ship a baseline for Latin and Cyrillic,
    // while allowing the renderer to fall back to installed fonts for other scripts.
    let fonts = options.fontdb_mut();
    for path in ["fonts/IBMPlexSans-Regular.ttf", "fonts/IBMPlexSans-Bold.ttf"] {
        let font = assets::Assets::get(path).with_context(|| format!("missing report font: {path}"))?;
        fonts.load_font_data(font.data.into_owned());
    }

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
    let height = 72 + lines.len() * 30;
    let too_large = "Report is too large for one image. Save it as PDF instead.";
    ensure!(height <= 32_000, "{too_large}");
    let height = height as u32;
    let mut svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{REPORT_WIDTH}" height="{height}"><g fill="#24262b" font-family="IBM Plex Sans,sans-serif">"##);
    for (i, (line, size, bold)) in lines.iter().enumerate() {
        svg.push_str(&format!(r#"<text x="40" y="{}" font-size="{size}" font-weight="{}">{}</text>"#, 42 + i * 30, if *bold { "bold" } else { "normal" }, escape(line)));
    }
    svg.push_str("</g></svg>");
    let tree = resvg::usvg::Tree::from_str(&svg, &options).context("cannot render the report")?;
    // The wrapping limit counts characters. Wide glyphs can still exceed the nominal width.
    let width = REPORT_WIDTH.max((tree.root().abs_bounding_box().right().ceil() as u32).saturating_add(40));
    ensure!(u64::from(width) * u64::from(height) <= MAX_REPORT_PIXELS, "{too_large}");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).context("cannot allocate the report image")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().context("cannot encode the report image")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_renders_multilingual_content_without_system_fonts() {
        let text = "Отчёт <edited> & tasks ".repeat(20);
        assert!(wrap(&text, 82).iter().all(|line| line.chars().count() <= 82));
        let bytes = render_png("Отчёт", &[("Completed".into(), vec![text])], resvg::usvg::Options::default()).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = resvg::tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!(decoded.width(), REPORT_WIDTH);
        let rows = decoded.pixels().chunks_exact(decoded.width() as usize);
        assert!(rows.clone().take(55).flatten().any(|pixel| pixel.red() < 150));
        assert!(rows.skip(55).flatten().any(|pixel| pixel.red() < 150));
    }

    #[test]
    fn image_preserves_wide_text() {
        let groups = [(String::new(), vec!["W".repeat(82)])];
        let bytes = render_png("", &groups, resvg::usvg::Options::default()).unwrap();
        let decoded = resvg::tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert!(decoded.width() > REPORT_WIDTH);
        assert!(decoded.pixels().chunks_exact(decoded.width() as usize)
            .any(|row| row[REPORT_WIDTH as usize..].iter().any(|pixel| pixel.red() < 150)));
    }
}
