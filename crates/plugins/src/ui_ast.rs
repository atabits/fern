//! Declarative UI AST (`UiNode`), callback handler identifiers, and SVG sanitization.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Maximum permitted raw SVG payload size (256 KiB).
pub const MAX_SVG_BYTES: usize = 256 * 1024;

/// Opaque callback identifier mapped to a JS closure inside the plugin's worker context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandlerId(pub u32);

/// Interactive UI event dispatched from the native GPUI layer back to the plugin worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiEvent {
    pub view_id: String,
    pub handler_id: HandlerId,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl UiEvent {
    pub fn click(view_id: impl Into<String>, handler_id: HandlerId) -> Self {
        Self {
            view_id: view_id.into(),
            handler_id,
            payload: serde_json::Value::Null,
        }
    }

    pub fn with_value(
        view_id: impl Into<String>,
        handler_id: HandlerId,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        Self {
            view_id: view_id.into(),
            handler_id,
            payload: value.into(),
        }
    }
}

/// Complete rendered tree delivered from a plugin view to the UI layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiTreePayload {
    pub plugin_id: String,
    pub view_id: String,
    pub root: UiNode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiAlign {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiJustify {
    #[default]
    Start,
    Center,
    End,
    Between,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiWeight {
    #[default]
    Normal,
    Medium,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiTextSize {
    Xs,
    Sm,
    #[default]
    Md,
    Lg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiButtonVariant {
    Primary,
    #[default]
    Secondary,
    Destructive,
    Ghost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiButtonSize {
    Sm,
    #[default]
    Md,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSelectOption {
    pub label: String,
    pub value: String,
}

fn default_cols() -> u16 {
    2
}

fn default_heading_level() -> u8 {
    2
}

fn default_svg_size() -> f32 {
    24.0
}

fn default_rows() -> u16 {
    4
}

/// Typed declarative UI primitives mapped to native GPUI components in `plugin_ui`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiNode {
    // Layout
    #[serde(rename = "vstack")]
    VStack {
        #[serde(default)]
        gap: Option<f32>,
        #[serde(default)]
        padding: Option<f32>,
        #[serde(default)]
        align: Option<UiAlign>,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    #[serde(rename = "hstack")]
    HStack {
        #[serde(default)]
        gap: Option<f32>,
        #[serde(default)]
        padding: Option<f32>,
        #[serde(default)]
        align: Option<UiAlign>,
        #[serde(default)]
        justify: Option<UiJustify>,
        #[serde(default)]
        wrap: bool,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    Grid {
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default)]
        gap: Option<f32>,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    Scroll {
        key: String,
        #[serde(default, alias = "maxHeight")]
        max_height: Option<f32>,
        #[serde(default)]
        child: Option<Box<UiNode>>,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    Card {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        padding: Option<f32>,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    Section {
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    Divider,
    Spacer {
        #[serde(default)]
        size: Option<f32>,
    },

    // Display
    Heading {
        text: String,
        #[serde(default = "default_heading_level")]
        level: u8,
    },
    Text {
        text: String,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        weight: Option<UiWeight>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        size: Option<UiTextSize>,
    },
    Markdown {
        content: String,
    },
    #[serde(alias = "keyValue")]
    KeyValue {
        label: String,
        value: String,
        #[serde(default)]
        muted: bool,
    },
    Badge {
        label: String,
        #[serde(default)]
        color: Option<String>,
    },
    Progress {
        value: f32,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    Donut {
        value: f32,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        color: Option<String>,
    },
    #[serde(alias = "emptyState")]
    EmptyState {
        #[serde(default)]
        icon: Option<String>,
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default, alias = "actionLabel")]
        action_label: Option<String>,
        #[serde(default, alias = "onAction")]
        on_action: Option<HandlerId>,
    },
    Svg {
        content: String,
        #[serde(default = "default_svg_size")]
        width: f32,
        #[serde(default = "default_svg_size")]
        height: f32,
    },

    // Input & Actions
    Button {
        label: String,
        #[serde(default)]
        variant: UiButtonVariant,
        #[serde(default)]
        size: UiButtonSize,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default)]
        loading: bool,
        #[serde(default, alias = "onClick")]
        on_click: Option<HandlerId>,
    },
    #[serde(alias = "iconButton")]
    IconButton {
        icon: String,
        #[serde(default)]
        tooltip: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onClick")]
        on_click: Option<HandlerId>,
    },
    Input {
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        revision: u64,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
        #[serde(default, alias = "onSubmit")]
        on_submit: Option<HandlerId>,
    },
    #[serde(alias = "textArea")]
    TextArea {
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        revision: u64,
        #[serde(default = "default_rows")]
        rows: u16,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
    },
    Select {
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<UiSelectOption>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
    },
    Checkbox {
        #[serde(default)]
        key: Option<String>,
        checked: bool,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
    },
    Switch {
        #[serde(default)]
        key: Option<String>,
        checked: bool,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        disabled: bool,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
    },
    #[serde(alias = "segmentedTabs")]
    SegmentedTabs {
        #[serde(default)]
        key: Option<String>,
        value: String,
        #[serde(default)]
        options: Vec<UiSelectOption>,
        #[serde(default, alias = "onChange")]
        on_change: Option<HandlerId>,
    },
    #[serde(alias = "listItem")]
    ListItem {
        key: String,
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        badge: Option<String>,
        #[serde(default)]
        trailing: Option<String>,
        #[serde(default)]
        selected: bool,
        #[serde(default, alias = "onClick")]
        on_click: Option<HandlerId>,
    },
}

impl UiNode {
    /// Recursively validates and sanitizes all embedded SVG nodes in this UI tree.
    pub fn sanitize_tree(&mut self) -> Result<()> {
        match self {
            Self::VStack { children, .. }
            | Self::HStack { children, .. }
            | Self::Grid { children, .. }
            | Self::Card { children, .. }
            | Self::Section { children, .. } => {
                for child in children {
                    child.sanitize_tree()?;
                }
            }
            Self::Scroll { child, children, .. } => {
                if let Some(c) = child {
                    c.sanitize_tree()?;
                }
                for c in children {
                    c.sanitize_tree()?;
                }
            }
            Self::Svg { content, .. } => {
                *content = sanitize_svg(content)?;
            }
            Self::Divider
            | Self::Spacer { .. }
            | Self::Heading { .. }
            | Self::Text { .. }
            | Self::Markdown { .. }
            | Self::KeyValue { .. }
            | Self::Badge { .. }
            | Self::Progress { .. }
            | Self::Donut { .. }
            | Self::EmptyState { .. }
            | Self::Button { .. }
            | Self::IconButton { .. }
            | Self::Input { .. }
            | Self::TextArea { .. }
            | Self::Select { .. }
            | Self::Checkbox { .. }
            | Self::Switch { .. }
            | Self::SegmentedTabs { .. }
            | Self::ListItem { .. } => {}
        }
        Ok(())
    }
}

/// Validates and sanitizes an untrusted SVG string before it reaches `usvg`/`resvg`.
///
/// Rejects payloads over [`MAX_SVG_BYTES`] (256 KiB) and strips `<!ENTITY>` / `<!DOCTYPE>`,
/// `<script>` / `<foreignObject>`, inline `on*` event attributes, external `<image>` / `<feImage>`,
/// and `javascript:` URIs.
pub fn sanitize_svg(raw: &str) -> Result<String> {
    ensure!(
        raw.len() <= MAX_SVG_BYTES,
        "SVG exceeds maximum size of {MAX_SVG_BYTES} bytes"
    );
    let cleaned = strip_dangerous_svg_constructs(raw);
    let lower = cleaned.to_ascii_lowercase();
    if !lower.contains("<svg") {
        bail!("SVG content missing <svg root element");
    }
    Ok(cleaned)
}

fn strip_dangerous_svg_constructs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            let ch_len = utf8_char_len(bytes[i]);
            let end = (i + ch_len).min(bytes.len());
            out.push_str(&input[i..end]);
            i = end;
            continue;
        }

        let rest = &input[i..];
        let rest_lower_prefix = prefix_ascii_lower(rest, 32);

        // Strip <!ENTITY ...> and <!DOCTYPE ...> (including [...] internal subset).
        if rest_lower_prefix.starts_with("<!entity")
            || rest_lower_prefix.starts_with("<!doctype")
        {
            i += skip_declaration(rest);
            continue;
        }

        // Strip dangerous block tags (<script>, <foreignObject>).
        if let Some(tag_len) = match_block_tag_start(&rest_lower_prefix, "script")
            .or_else(|| match_block_tag_start(&rest_lower_prefix, "foreignobject"))
        {
            let tag_name = if rest_lower_prefix.starts_with("<script") {
                "script"
            } else {
                "foreignobject"
            };
            let _ = tag_len;
            i += skip_element_block(rest, tag_name);
            continue;
        }

        // Strip external <image ...> / <feImage ...> tags.
        if match_tag_name_start(&rest_lower_prefix, "image")
            || match_tag_name_start(&rest_lower_prefix, "feimage")
        {
            let tag_end = find_tag_end(rest);
            let tag_slice = &rest[..tag_end];
            if tag_has_external_ref(tag_slice) {
                let tag_name = if match_tag_name_start(&rest_lower_prefix, "image") {
                    "image"
                } else {
                    "feimage"
                };
                if tag_slice.trim_end().ends_with("/>") {
                    i += tag_end;
                } else {
                    i += skip_element_block(rest, tag_name);
                }
                continue;
            }
        }

        // Normal tag: strip any `on*=` attributes and `javascript:`/`vbscript:` href/src attributes.
        let tag_end = find_tag_end(rest);
        let tag_slice = &rest[..tag_end];
        out.push_str(&sanitize_tag_attributes(tag_slice));
        i += tag_end;
    }

    out
}

fn utf8_char_len(first_byte: u8) -> usize {
    match first_byte {
        0..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

fn prefix_ascii_lower(s: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if out.len() >= max_bytes {
            break;
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn match_tag_name_start(lower_prefix: &str, tag: &str) -> bool {
    let Some(after) = lower_prefix.strip_prefix('<') else {
        return false;
    };
    let Some(tail) = after.strip_prefix(tag) else {
        return false;
    };
    tail.is_empty()
        || tail.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
}

fn match_block_tag_start(lower_prefix: &str, tag: &str) -> Option<usize> {
    if match_tag_name_start(lower_prefix, tag) {
        Some(tag.len() + 1)
    } else {
        None
    }
}

fn skip_declaration(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    let mut bracket_depth = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else {
            match b {
                b'"' | b'\'' => quote = Some(b),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'>' if bracket_depth == 0 => return i + 1,
                _ => {}
            }
        }
        i += 1;
    }
    bytes.len()
}

fn find_tag_end(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
        } else if b == b'"' || b == b'\'' {
            quote = Some(b);
        } else if b == b'>' {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

fn skip_element_block(rest: &str, tag_name_lower: &str) -> usize {
    let open_end = find_tag_end(rest);
    if rest[..open_end].trim_end().ends_with("/>") {
        return open_end;
    }
    let close_needle = format!("</{tag_name_lower}");
    let mut i = open_end;
    while i < rest.len() {
        let tail = &rest[i..];
        if let Some(pos) = tail.find('<') {
            i += pos;
            let candidate = prefix_ascii_lower(&rest[i..], close_needle.len() + 4);
            if let Some(after) = candidate.strip_prefix(&close_needle)
                && (after.is_empty()
                    || after.starts_with(|c: char| c.is_ascii_whitespace() || c == '>'))
            {
                let close_end = find_tag_end(&rest[i..]);
                return i + close_end;
            }
            i += 1;
        } else {
            return rest.len();
        }
    }
    rest.len()
}

fn tag_has_external_ref(tag: &str) -> bool {
    let attrs = parse_tag_attributes(tag);
    attrs.into_iter().any(|(_, name, val)| {
        let name_lower = name.to_ascii_lowercase();
        if matches!(name_lower.as_str(), "href" | "xlink:href" | "src") {
            let v = val.trim().to_ascii_lowercase();
            !(v.is_empty() || v.starts_with('#') || v.starts_with("data:image/"))
                || v.starts_with("data:image/svg")
        } else {
            false
        }
    })
}

fn sanitize_tag_attributes(tag: &str) -> String {
    if !tag.contains('=') {
        return tag.to_owned();
    }
    let bytes = tag.as_bytes();
    let Some(first_ws) = bytes.iter().position(|b| b.is_ascii_whitespace()) else {
        return tag.to_owned();
    };
    let head = &tag[..first_ws];
    let self_closing = tag.trim_end().ends_with("/>");

    let mut out = String::from(head);
    for (_, name, raw_val_with_quotes) in parse_tag_attributes_raw(tag) {
        let name_lower = name.to_ascii_lowercase();
        if name_lower.starts_with("on") && name_lower.len() > 2 {
            continue;
        }
        let unquoted = unquote_attr(raw_val_with_quotes).trim().to_ascii_lowercase();
        if matches!(name_lower.as_str(), "href" | "xlink:href" | "src")
            && (unquoted.starts_with("javascript:")
                || unquoted.starts_with("vbscript:")
                || unquoted.starts_with("data:text/html")
                || unquoted.starts_with("http://")
                || unquoted.starts_with("https://")
                || unquoted.starts_with("//")
                || unquoted.starts_with("file://"))
        {
            continue;
        }
        out.push(' ');
        out.push_str(name);
        if !raw_val_with_quotes.is_empty() {
            out.push('=');
            out.push_str(raw_val_with_quotes);
        }
    }
    if self_closing {
        out.push_str("/>");
    } else if tag.trim_end().ends_with('>') {
        out.push('>');
    }
    out
}

fn unquote_attr(val: &str) -> &str {
    let trimmed = val.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn parse_tag_attributes(tag: &str) -> Vec<(usize, &str, String)> {
    parse_tag_attributes_raw(tag)
        .into_iter()
        .map(|(pos, name, raw)| (pos, name, unquote_attr(raw).to_owned()))
        .collect()
}

fn parse_tag_attributes_raw(tag: &str) -> Vec<(usize, &str, &str)> {
    let bytes = tag.as_bytes();
    let Some(mut i) = bytes.iter().position(|b| b.is_ascii_whitespace()) else {
        return Vec::new();
    };
    let end = if tag.trim_end().ends_with("/>") {
        tag.rfind("/>").unwrap_or(bytes.len())
    } else if tag.trim_end().ends_with('>') {
        tag.rfind('>').unwrap_or(bytes.len())
    } else {
        bytes.len()
    };

    let mut attrs = Vec::new();
    while i < end {
        while i < end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= end {
            break;
        }
        let name_start = i;
        while i < end && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let name = &tag[name_start..i];
        if name.is_empty() {
            i += 1;
            continue;
        }
        while i < end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < end && bytes[i] == b'=' {
            i += 1;
            while i < end && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let val_start = i;
            if i < end && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                while i < end && bytes[i] != q {
                    i += 1;
                }
                if i < end {
                    i += 1;
                }
            } else {
                while i < end && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            attrs.push((name_start, name, &tag[val_start..i]));
        } else {
            attrs.push((name_start, name, ""));
        }
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_sanitizer_strips_xss_xxe_and_external_images_and_caps_size() {
        let dirty = r#"<!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
            <svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" viewBox="0 0 24 24">
                <script>alert('xss')</script>
                <image href="https://evil.example.com/track.png" width="10" height="10"/>
                <circle cx="12" cy="12" r="8" onclick="evil()" fill="currentColor"/>
            </svg>"#;
        let clean = sanitize_svg(dirty).unwrap();
        let lower = clean.to_ascii_lowercase();
        assert!(!lower.contains("<!entity"));
        assert!(!lower.contains("<!doctype"));
        assert!(!lower.contains("<script"));
        assert!(!lower.contains("onload"));
        assert!(!lower.contains("onclick"));
        assert!(!lower.contains("evil.example.com"));
        assert!(lower.contains("<circle"));

        let oversized = format!("<svg>{}</svg>", "a".repeat(MAX_SVG_BYTES + 1));
        assert!(sanitize_svg(&oversized).is_err());
    }
}
