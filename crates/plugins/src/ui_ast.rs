//! Declarative UI AST (`UiNode`), callback handler identifiers, and SVG sanitization.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Maximum permitted raw SVG payload size (256 KiB).
pub const MAX_SVG_BYTES: usize = 256 * 1024;
const MAX_SVG_NODES: u32 = 4096;
const MAX_SVG_DEPTH: usize = 64;
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

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
/// Parses strictly with `roxmltree` (`allow_dtd: false`) and rebuilds clean SVG XML using local
/// XML element/attribute names:
/// - Rejects payloads over [`MAX_SVG_BYTES`] (256 KiB), gzip/SVGZ magic bytes, NUL/control bytes,
///   DTDs (`<!DOCTYPE>`/`<!ENTITY>`), XML processing instructions, non-`<svg>` roots, and
///   excessive nesting/node counts.
/// - Strips comments, foreign-namespace elements/attributes, `<script>`, `<foreignObject>`,
///   `<image>`, `<feImage>`, `<style>`, `<animate*>`, `<set>`, `<a>`, all `on*` event attributes,
///   any `href`/`xlink:href`/`src` attribute not starting with `#`, and any external `url(...)`.
pub fn sanitize_svg(raw: &str) -> Result<String> {
    ensure!(
        raw.len() <= MAX_SVG_BYTES,
        "SVG exceeds maximum size of {MAX_SVG_BYTES} bytes"
    );
    let trimmed = raw.strip_prefix('\u{feff}').unwrap_or(raw).trim();
    ensure!(!trimmed.is_empty(), "SVG content is empty");
    ensure!(
        !trimmed.as_bytes().starts_with(&[0x1f, 0x8b]),
        "compressed SVGZ payloads are forbidden"
    );
    ensure!(
        !trimmed
            .bytes()
            .any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')),
        "SVG contains forbidden control bytes"
    );

    let doc = roxmltree::Document::parse_with_options(
        trimmed,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_SVG_NODES,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|err| anyhow::anyhow!("invalid or disallowed SVG XML: {err}"))?;

    if doc.descendants().any(|n| n.is_pi()) {
        bail!("XML processing instructions are forbidden in SVG");
    }

    let root = doc.root_element();
    ensure!(
        root.tag_name().name() == "svg",
        "SVG content missing <svg> root element"
    );
    ensure!(
        is_allowed_svg_ns(root.tag_name().namespace()),
        "SVG root element uses a disallowed XML namespace"
    );

    let mut uses_xlink = false;
    let mut root_attrs = String::new();
    write_sanitized_attributes(root, &mut root_attrs, &mut uses_xlink);

    let mut body = String::new();
    for child in root.children() {
        write_sanitized_node(child, 1, &mut body, &mut uses_xlink)?;
    }

    let mut out = String::with_capacity(trimmed.len().min(MAX_SVG_BYTES));
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\"");
    if uses_xlink {
        out.push_str(" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
    }
    out.push_str(&root_attrs);
    if body.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        out.push_str(&body);
        out.push_str("</svg>");
    }

    ensure!(
        out.len() <= MAX_SVG_BYTES,
        "sanitized SVG exceeds maximum size of {MAX_SVG_BYTES} bytes"
    );
    Ok(out)
}

fn is_allowed_svg_ns(ns: Option<&str>) -> bool {
    matches!(ns, None | Some(SVG_NS))
}

fn is_safe_xml_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_forbidden_svg_element(local_lower: &str) -> bool {
    local_lower.starts_with("animate")
        || matches!(
            local_lower,
            "script"
                | "foreignobject"
                | "image"
                | "feimage"
                | "style"
                | "set"
                | "discard"
                | "mpath"
                | "handler"
                | "a"
                | "iframe"
                | "object"
                | "embed"
                | "audio"
                | "video"
                | "canvas"
                | "applet"
                | "link"
                | "meta"
                | "base"
                | "metadata"
        )
}

fn is_safe_fragment_href(raw: &str) -> bool {
    let trimmed = raw.trim();
    let Some(id) = trimmed.strip_prefix('#') else {
        return false;
    };
    !id.is_empty()
        && id.len() <= 256
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

fn has_only_safe_urls(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains("javascript:")
        || lower.contains("vbscript:")
        || lower.contains("data:")
        || lower.contains("file:")
        || lower.contains("\\\\")
    {
        return false;
    }

    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"url") {
            let prev_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-' || bytes[i - 1] == b'_');
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if prev_ok && j < bytes.len() && bytes[j] == b'(' {
                let Some(rel_close) = lower[j + 1..].find(')') else {
                    return false;
                };
                let inner = value[j + 1..j + 1 + rel_close].trim();
                let unquoted = inner
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(inner)
                    .trim();
                if !is_safe_fragment_href(unquoted) {
                    return false;
                }
                i = j + 1 + rel_close + 1;
                continue;
            }
        }
        i += 1;
    }
    true
}

fn is_safe_inline_style(value: &str) -> bool {
    if value.contains('\\') || value.contains("/*") {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("@import")
        || lower.contains("expression")
        || lower.contains("behavior")
        || lower.contains("-moz-binding")
    {
        return false;
    }
    has_only_safe_urls(value)
}

fn write_sanitized_node(
    node: roxmltree::Node<'_, '_>,
    depth: usize,
    out: &mut String,
    uses_xlink: &mut bool,
) -> Result<()> {
    ensure!(
        depth <= MAX_SVG_DEPTH,
        "SVG nesting exceeds maximum depth of {MAX_SVG_DEPTH}"
    );
    match node.node_type() {
        roxmltree::NodeType::Comment | roxmltree::NodeType::Root => Ok(()),
        roxmltree::NodeType::PI => bail!("XML processing instructions are forbidden in SVG"),
        roxmltree::NodeType::Text => {
            if let Some(text) = node.text() {
                push_escaped_xml_text(out, text);
            }
            Ok(())
        }
        roxmltree::NodeType::Element => {
            if !is_allowed_svg_ns(node.tag_name().namespace()) {
                return Ok(());
            }
            let local = node.tag_name().name();
            if !is_safe_xml_ident(local) {
                return Ok(());
            }
            let local_lower = local.to_ascii_lowercase();
            if is_forbidden_svg_element(&local_lower) {
                return Ok(());
            }

            let mut attrs = String::new();
            write_sanitized_attributes(node, &mut attrs, uses_xlink);

            let mut children_buf = String::new();
            for child in node.children() {
                write_sanitized_node(child, depth + 1, &mut children_buf, uses_xlink)?;
            }

            out.push('<');
            out.push_str(local);
            out.push_str(&attrs);
            if children_buf.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                out.push_str(&children_buf);
                out.push_str("</");
                out.push_str(local);
                out.push('>');
            }
            Ok(())
        }
    }
}

fn write_sanitized_attributes(
    node: roxmltree::Node<'_, '_>,
    out: &mut String,
    uses_xlink: &mut bool,
) {
    let mut emitted_names: Vec<String> = Vec::new();
    for attr in node.attributes() {
        let local = attr.name();
        if !is_safe_xml_ident(local) {
            continue;
        }
        let local_lower = local.to_ascii_lowercase();
        if local_lower.starts_with("on") {
            continue;
        }
        let val = attr.value();

        let (emitted_name, emitted_val) = match attr.namespace() {
            None | Some(SVG_NS) => {
                if local_lower == "href" || local_lower == "src" {
                    let trimmed = val.trim();
                    if !is_safe_fragment_href(trimmed) {
                        continue;
                    }
                    (local.to_owned(), trimmed)
                } else if local_lower == "style" {
                    if !is_safe_inline_style(val) {
                        continue;
                    }
                    (local.to_owned(), val)
                } else {
                    if !has_only_safe_urls(val) {
                        continue;
                    }
                    (local.to_owned(), val)
                }
            }
            Some(XLINK_NS) => {
                if local_lower != "href" {
                    continue;
                }
                let trimmed = val.trim();
                if !is_safe_fragment_href(trimmed) {
                    continue;
                }
                *uses_xlink = true;
                ("xlink:href".to_owned(), trimmed)
            }
            Some(XML_NS) => {
                if !matches!(local_lower.as_str(), "space" | "lang" | "id") {
                    continue;
                }
                if !has_only_safe_urls(val) {
                    continue;
                }
                (format!("xml:{local}"), val)
            }
            Some(_) => continue,
        };

        if emitted_names.iter().any(|existing| existing == &emitted_name) {
            continue;
        }
        emitted_names.push(emitted_name.clone());

        out.push(' ');
        out.push_str(&emitted_name);
        out.push_str("=\"");
        push_escaped_xml_attr(out, emitted_val);
        out.push('"');
    }
}

fn push_escaped_xml_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn push_escaped_xml_attr(out: &mut String, val: &str) {
    for ch in val.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_sanitizer_rejects_dtd_pi_svgz_and_strips_prefixed_xss_unc_and_data_refs() {
        // DTD / XXE must be rejected outright by roxmltree allow_dtd: false.
        let dtd_svg = r#"<!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8"/></svg>"#;
        assert!(sanitize_svg(dtd_svg).is_err());

        // XML processing instruction (other than XML declaration) must be rejected.
        let pi_svg = r#"<?xml-stylesheet type="text/xsl" href="http://evil/x.xsl"?>
            <svg xmlns="http://www.w3.org/2000/svg"><circle cx="5" cy="5" r="4"/></svg>"#;
        assert!(sanitize_svg(pi_svg).is_err());

        // XML declaration is fine for standard SVG icons.
        let xml_decl_svg = r#"<?xml version="1.0" encoding="UTF-8"?>
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
                <path d="M21 12a9 9 0 1 1-4.5-7.8"/>
            </svg>"#;
        let clean_decl = sanitize_svg(xml_decl_svg).unwrap();
        assert!(clean_decl.contains("<path d=\"M21 12a9 9 0 1 1-4.5-7.8\"/>"));

        // Prefixed <s:image>, custom XLink prefix x:href, UNC paths, raw local paths, data: SVGZ,
        // comment quote-desync, <style>, <animate*>, <set>, <foreignObject>, and on* attrs.
        let dirty = r##"<!-- unpaired quote " -->
            <s:svg xmlns:s="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/1999/xlink" onload="alert(1)" viewBox="0 0 24 24">
                <!-- " --><s:script>alert('xss')</s:script>
                <s:foreignObject><div xmlns="http://www.w3.org/1999/xhtml">html</div></s:foreignObject>
                <s:style>@import url("http://evil.example/a.css");</s:style>
                <s:image x:href="\\evil.host\share\leak.png" width="10" height="10"/>
                <s:image href="C:\Windows\win.ini" width="10" height="10"/>
                <s:image href="/etc/passwd" width="10" height="10"/>
                <s:image href="data:image/svg+xml;base64,H4sIAAAAAAAA" width="10" height="10"/>
                <s:feImage x:href="data:text/plain;base64,H4sIAAAAAAAA"/>
                <s:use x:href="\\evil.host\share\x.svg#id"/>
                <s:use href="&#106;avascript:alert(1)"/>
                <s:use x:href="#ok_grad"/>
                <s:animate attributeName="x" from="0" to="10"/>
                <s:set attributeName="fill" to="red"/>
                <s:defs>
                    <s:linearGradient id="ok_grad"><s:stop offset="0" stop-color="#45C686"/></s:linearGradient>
                </s:defs>
                <s:circle cx="12" cy="12" r="8" onclick="evil()" fill="url(#ok_grad)"/>
                <s:rect x="0" y="0" width="4" height="4" fill="url(http://evil.example/g.svg#x)"/>
            </s:svg>"##;
        let clean = sanitize_svg(dirty).unwrap();
        let lower = clean.to_ascii_lowercase();
        assert!(!lower.contains("<!--"));
        assert!(!lower.contains("script"));
        assert!(!lower.contains("foreignobject"));
        assert!(!lower.contains("<style"));
        assert!(!lower.contains("<image"));
        assert!(!lower.contains("feimage"));
        assert!(!lower.contains("animate"));
        assert!(!lower.contains("<set"));
        assert!(!lower.contains("onload"));
        assert!(!lower.contains("onclick"));
        assert!(!lower.contains("evil.host"));
        assert!(!lower.contains("win.ini"));
        assert!(!lower.contains("/etc/passwd"));
        assert!(!lower.contains("data:"));
        assert!(!lower.contains("javascript:"));
        assert!(!lower.contains("evil.example"));
        assert!(clean.contains("xlink:href=\"#ok_grad\""));
        assert!(clean.contains("fill=\"url(#ok_grad)\""));
        assert!(lower.contains("<circle"));

        // Raw local/UNC path, data URI, and control/SVGZ bytes must fail outright.
        assert!(sanitize_svg(r"\\evil.host\share\icon.svg").is_err());
        assert!(sanitize_svg("data:image/svg+xml,<svg/>").is_err());
        assert!(sanitize_svg("\x1f\x08\x00<svg/>").is_err());

        // Size cap enforcement.
        let oversized = format!("<svg>{}</svg>", "a".repeat(MAX_SVG_BYTES + 1));
        assert!(sanitize_svg(&oversized).is_err());

        // Real TickTick icon renders cleanly.
        let ticktick = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 12a9 9 0 1 1-4.5-7.8" />
          <path d="M9 12.2l2.6 2.6L21 5.4" />
        </svg>"#;
        let clean_tt = sanitize_svg(ticktick).unwrap();
        assert!(clean_tt.contains("M21 12a9 9 0 1 1-4.5-7.8"));
        assert!(clean_tt.contains("M9 12.2l2.6 2.6L21 5.4"));
    }
}
