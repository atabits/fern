//! Markdown formatting for an edited text, as the buttons of the original's editor toolbar do
//! it: the selection is wrapped in markers (`**bold**`), or its lines get a prefix (`> quote`).
//! Applying a format that is already there takes it off again.

use std::ops::Range;

/// A toolbar button. The markers are the original's (its tooltips show them: `::Highlight::`,
/// `~Underline~`, `- [ ] Check Item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A heading of level 1 to 3.
    Heading(u8),
    Bold,
    Highlight,
    Checklist,
    Bullets,
    Numbered,
    Italic,
    Underline,
    Strike,
    Divider,
    Link,
    Code,
    Quote,
}

impl Format {
    fn markers(self) -> Option<&'static str> {
        Some(match self {
            Self::Bold => "**",
            Self::Highlight => "::",
            Self::Italic => "*",
            Self::Underline => "~",
            Self::Strike => "~~",
            Self::Code => "`",
            _ => return None,
        })
    }

    /// A line prefix: its text for the line numbered `n` (from 1), or `None` for inline formats.
    fn prefix(self, n: usize) -> Option<String> {
        Some(match self {
            Self::Heading(level) => format!("{} ", "#".repeat(level.clamp(1, 6) as usize)),
            Self::Checklist => "- [ ] ".to_owned(),
            Self::Bullets => "* ".to_owned(),
            Self::Numbered => format!("{n}. "),
            Self::Quote => "> ".to_owned(),
            _ => return None,
        })
    }
}

/// Applies `format` to `selection` (byte offsets) of `text`: the new text and the new selection.
pub fn apply(text: &str, selection: Range<usize>, format: Format) -> (String, Range<usize>) {
    let selection = clamp(text, selection);
    if let Some(markers) = format.markers() {
        return wrap(text, selection, markers);
    }
    match format {
        Format::Link => link(text, selection),
        Format::Divider => divider(text, selection),
        _ => prefix_lines(text, selection, format),
    }
}

/// Replaces the selection with `inserted`, leaving the cursor after it (the Time button).
pub fn insert(text: &str, selection: Range<usize>, inserted: &str) -> (String, Range<usize>) {
    let selection = clamp(text, selection);
    let new = format!("{}{inserted}{}", &text[..selection.start], &text[selection.end..]);
    let cursor = selection.start + inserted.len();
    (new, cursor..cursor)
}

fn clamp(text: &str, selection: Range<usize>) -> Range<usize> {
    let fit = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (start, end) = (fit(selection.start), fit(selection.end));
    start.min(end)..start.max(end)
}

fn wrap(text: &str, selection: Range<usize>, markers: &str) -> (String, Range<usize>) {
    let (before, inner, after) = (&text[..selection.start], &text[selection.clone()], &text[selection.end..]);
    let len = markers.len();
    // Already wrapped, the markers outside the selection or inside it: unwrap.
    if before.ends_with(markers) && after.starts_with(markers) && !single_of_double(before, after, markers) {
        let new = format!("{}{inner}{}", &before[..before.len() - len], &after[len..]);
        return (new, selection.start - len..selection.end - len);
    }
    if inner.len() >= 2 * len && inner.starts_with(markers) && inner.ends_with(markers) {
        let new = format!("{before}{}{after}", &inner[len..inner.len() - len]);
        return (new, selection.start..selection.end - 2 * len);
    }
    let new = format!("{before}{markers}{inner}{markers}{after}");
    (new, selection.start + len..selection.end + len)
}

/// Whether one `*` or `~` around the selection belongs to a doubled marker (`**`, `~~`), which
/// is another format.
fn single_of_double(before: &str, after: &str, markers: &str) -> bool {
    markers.len() == 1
        && before.len() >= 2
        && after.len() >= 2
        && before[..before.len() - 1].ends_with(markers)
        && after[1..].starts_with(markers)
}

fn link(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
    let (before, inner, after) = (&text[..selection.start], &text[selection.clone()], &text[selection.end..]);
    let new = format!("{before}[{inner}](){after}");
    // The cursor goes where there is something to type: the URL, or the text when there is none.
    let cursor = if inner.is_empty() { selection.start + 1 } else { selection.end + 3 };
    (new, cursor..cursor)
}

fn divider(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
    // The line goes under the cursor's line, or takes its place when that one is empty.
    let line_end = text[selection.end..].find('\n').map_or(text.len(), |i| selection.end + i);
    let line_start = text[..line_end].rfind('\n').map_or(0, |i| i + 1);
    let (at, inserted) = if text[line_start..line_end].trim().is_empty() { (line_start, "---") } else { (line_end, "\n---") };
    let new = format!("{}{inserted}{}", &text[..at], &text[at..]);
    let cursor = at + inserted.len();
    (new, cursor..cursor)
}

/// The prefix a line has for `format`, if any: the format's own, or for headings and
/// numbers any level or number.
fn existing_prefix(line: &str, format: Format) -> Option<usize> {
    match format {
        Format::Heading(_) => {
            let hashes = line.len() - line.trim_start_matches('#').len();
            (hashes > 0 && line[hashes..].starts_with(' ')).then_some(hashes + 1)
        }
        Format::Numbered => {
            let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            (digits > 0 && line[digits..].starts_with(". ")).then_some(digits + 2)
        }
        _ => format.prefix(1).filter(|p| line.starts_with(p.as_str())).map(|p| p.len()),
    }
}

fn prefix_lines(text: &str, selection: Range<usize>, format: Format) -> (String, Range<usize>) {
    let start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[selection.end..].find('\n').map_or(text.len(), |i| selection.end + i);
    let lines: Vec<&str> = text[start..end].split('\n').collect();
    // A heading of the same level, or every line already formatted, is taken off.
    let remove = lines.iter().all(|line| match format {
        Format::Heading(_) => existing_prefix(line, format).is_some_and(|len| line[..len] == *format.prefix(1).unwrap()),
        _ => existing_prefix(line, format).is_some(),
    });
    let mut n = 0;
    let changed: Vec<String> = lines
        .iter()
        .map(|line| {
            if remove {
                return line[existing_prefix(line, format).unwrap_or(0)..].to_owned();
            }
            n += 1;
            // A line formatted already, or a heading of another level, is not formatted twice.
            let rest = &line[existing_prefix(line, format).unwrap_or(0)..];
            format!("{}{rest}", format.prefix(n).unwrap())
        })
        .collect();
    let block = changed.join("\n");
    let new = format!("{}{block}{}", &text[..start], &text[end..]);
    // One line keeps the cursor at the same place in its text; several are selected whole.
    let range = if lines.len() == 1 {
        let shift = block.len() as isize - lines[0].len() as isize;
        let move_by = |i: usize| (i as isize + shift).clamp(start as isize, (start + block.len()) as isize) as usize;
        move_by(selection.start)..move_by(selection.end)
    } else {
        start..start + block.len()
    };
    (new, range)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, selection: Range<usize>, format: Format) -> (String, String) {
        let (new, range) = apply(text, selection, format);
        let selected = new[range].to_owned();
        (new, selected)
    }

    #[test]
    fn inline_markers() {
        assert_eq!(run("say hi now", 4..6, Format::Bold), ("say **hi** now".into(), "hi".into()));
        assert_eq!(run("say **hi** now", 6..8, Format::Bold), ("say hi now".into(), "hi".into()), "toggled off");
        assert_eq!(run("say **hi** now", 4..10, Format::Bold), ("say hi now".into(), "hi".into()));
        assert_eq!(run("a b", 2..3, Format::Highlight).0, "a ::b::");
        assert_eq!(run("a b", 2..3, Format::Underline).0, "a ~b~");
        assert_eq!(run("a b", 2..3, Format::Strike).0, "a ~~b~~");
        assert_eq!(run("a b", 2..3, Format::Code).0, "a `b`");
        // Italic inside bold adds a marker rather than taking bold's apart.
        assert_eq!(run("**b**", 2..3, Format::Italic).0, "***b***");
        let (new, range) = apply("ab", 1..1, Format::Italic);
        assert_eq!((new.as_str(), range), ("a**b", 2..2), "an empty selection gets the cursor inside");
    }

    #[test]
    fn line_prefixes() {
        assert_eq!(run("one\ntwo\nthree", 1..6, Format::Numbered), ("1. one\n2. two\nthree".into(), "1. one\n2. two".into()));
        assert_eq!(run("1. one\n2. two", 0..9, Format::Numbered).0, "one\ntwo");
        assert_eq!(run("a\n- [ ] b", 0..9, Format::Checklist).0, "- [ ] a\n- [ ] b", "a mixed block is all formatted");
        assert_eq!(run("todo", 2..2, Format::Bullets), ("* todo".into(), "".into()));
        assert_eq!(apply("todo", 2..2, Format::Quote), ("> todo".into(), 4..4));
        assert_eq!(run("# Title", 3..3, Format::Heading(2)).0, "## Title", "another level replaces it");
        assert_eq!(run("## Title", 3..3, Format::Heading(2)).0, "Title", "the same level takes it off");
    }

    #[test]
    fn links_dividers_and_time() {
        assert_eq!(apply("see docs", 4..8, Format::Link), ("see [docs]()".into(), 11..11));
        assert_eq!(apply("x", 1..1, Format::Link), ("x[]()".into(), 2..2));
        assert_eq!(apply("one\ntwo", 1..1, Format::Divider), ("one\n---\ntwo".into(), 7..7));
        assert_eq!(apply("one\n", 4..4, Format::Divider), ("one\n---".into(), 7..7));
        assert_eq!(insert("at ", 3..3, "14:05"), ("at 14:05".into(), 8..8));
        assert_eq!(insert("é", 1..9, "x").0, "x", "a selection inside a character or past the end is clamped");
    }
}
