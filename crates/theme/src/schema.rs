use std::path::Path;

use anyhow::{Context as _, Result};
use gpui::{Background, Hsla, SharedString, linear_color_stop, linear_gradient, rgb};
use serde::{Deserialize, Serialize};

use crate::{
    Appearance, StatusColors, Theme, ThemeColors, ThemePreset, foreground_on, presets,
    readable_accent,
};

pub const EXAMPLE_THEME_TEMPLATE: &str = r##"// Fern custom themes — JSONC format.
// Drop .json or .jsonc files into this folder; saved changes reload automatically.
//
// Supported features:
// - Theme family files ("themes": [...]) or a single theme object
// - Partial overrides: only specify the colors you want to change; others derive automatically
// - Optional "base": inherit from any built-in Fern preset ("night", "grey", "matcha", "darkpurple", ...)
// - Colors in #rgb, #rgba, #rrggbb, or #rrggbbaa hex format
// - Optional gradients for sidebar/lists/calendar/matrix backgrounds: ["#top", "#bottom"]
//   or { "angle": 180, "stops": ["#top", "#bottom"] }
{
  "name": "Fern Custom Example",
  "author": "You",
  "themes": [
    {
      "id": "fern-dusk",
      "name": "Fern Dusk",
      "appearance": "dark",
      "swatch": ["#7aa2f7", "#1a1b26"],
      "style": {
        "background": "#1a1b26",
        "panel.background": "#16161e",
        "surface.background": "#24283b",
        "elevated_surface.background": "#24283b",
        "text": "#c0caf5",
        "accent": "#7aa2f7",
        "sidebar.background": ["#24283b", "#16161e"],
        "error": "#f7768e",
        "warning": "#e0af68",
        "info": "#7dcfff",
        "success": "#9ece6a"
      }
    },
    {
      "id": "fern-paper",
      "name": "Fern Paper",
      "appearance": "light",
      "swatch": ["#c05d3b", "#f5f0e6"],
      "style": {
        "background": "#faf7f2",
        "panel.background": "#f2ece1",
        "surface.background": "#f1ebe0",
        "elevated_surface.background": "#fffdf9",
        "sidebar.background": ["#efe8dc", "#e8dfd0"],
        "sidebar.border": "#dfd6c7",
        "sidebar.icon": "#c05d3b",
        "sidebar.icon_hover": "#2b241fb8",
        "sidebar.icon_muted": "#2b241f78",
        "sidebar.icon_bottom": "#2b241f6b",
        "avatar.background": "#e3dacb",
        "avatar.icon": "#807366",
        "lists.background": "#f5f0e7",
        "lists.border": "#e6ddd0",
        "list.text": "#2b241f",
        "list.text_muted": "#73675c",
        "list.count": "#86796d",
        "list.hover": "#c05d3b12",
        "list.selected": "#c05d3b1f",
        "calendar.background": "#faf7f2",
        "matrix.background": "#f4efe6",
        "border": "#ded5c7",
        "border.variant": "#eae2d6",
        "element.hover": "#eae2d5",
        "element.selected": "#c05d3b1f",
        "ghost_element.hover": "#2b241f0b",
        "ghost_element.selected": "#c05d3b16",
        "text": "#2b241f",
        "text.muted": "#766a5f",
        "text.placeholder": "#7d7166",
        "icon": "#3d332c",
        "icon.muted": "#7d7166",
        "checkbox.border": "#a6998c",
        "accent": "#c05d3b",
        "error": "#c94a38",
        "warning": "#c98320",
        "info": "#4b7499",
        "success": "#4a8556"
      }
    }
  ]
}
"##;

fn default_gradient_angle() -> f32 {
    180.0
}

/// Parses `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa` into [`Hsla`].
pub fn try_parse_color(value: &str) -> Result<Hsla> {
    Ok(gpui::Rgba::try_from(value)?.into())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SwatchContent {
    Solid(String),
    Gradient([String; 2]),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BackgroundContent {
    Solid(String),
    Vertical([String; 2]),
    Gradient {
        #[serde(default = "default_gradient_angle")]
        angle: f32,
        stops: [String; 2],
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerColorContent {
    pub cursor: Option<String>,
    pub background: Option<String>,
    pub selection: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeFamilyContent {
    pub name: String,
    #[serde(default)]
    pub author: String,
    pub themes: Vec<ThemeContent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeContent {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    pub appearance: Appearance,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub swatch: Option<SwatchContent>,
    #[serde(default)]
    pub style: ThemeStyleContent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeStyleContent {
    pub background: Option<String>,
    #[serde(rename = "editor.background", alias = "editor_background")]
    pub editor_background: Option<String>,

    #[serde(rename = "sidebar.background", alias = "sidebar_background")]
    pub sidebar_background: Option<BackgroundContent>,
    #[serde(rename = "sidebar.border", alias = "sidebar_border")]
    pub sidebar_border: Option<String>,
    #[serde(rename = "sidebar.icon", alias = "sidebar_icon")]
    pub sidebar_icon: Option<String>,
    #[serde(rename = "sidebar.icon_hover", alias = "sidebar_icon_hover")]
    pub sidebar_icon_hover: Option<String>,
    #[serde(rename = "sidebar.icon_muted", alias = "sidebar_icon_muted")]
    pub sidebar_icon_muted: Option<String>,
    #[serde(rename = "sidebar.icon_bottom", alias = "sidebar_icon_bottom")]
    pub sidebar_icon_bottom: Option<String>,

    #[serde(rename = "avatar.background", alias = "avatar_background")]
    pub avatar_background: Option<String>,
    #[serde(rename = "avatar.icon", alias = "avatar_icon")]
    pub avatar_icon: Option<String>,

    #[serde(rename = "lists.background", alias = "lists_background")]
    pub lists_background: Option<BackgroundContent>,
    #[serde(rename = "lists.border", alias = "lists_border")]
    pub lists_border: Option<String>,
    #[serde(rename = "list.text", alias = "list_text")]
    pub list_text: Option<String>,
    #[serde(rename = "list.text_muted", alias = "list_text_muted")]
    pub list_text_muted: Option<String>,
    #[serde(rename = "list.count", alias = "list_count")]
    pub list_count: Option<String>,
    #[serde(rename = "list.hover", alias = "list_hover")]
    pub list_hover: Option<String>,
    #[serde(rename = "list.selected", alias = "list_selected")]
    pub list_selected: Option<String>,

    #[serde(rename = "calendar.background", alias = "calendar_background")]
    pub calendar_background: Option<BackgroundContent>,
    #[serde(rename = "matrix.background", alias = "matrix_background")]
    pub matrix_background: Option<BackgroundContent>,

    #[serde(rename = "panel.background", alias = "panel_background")]
    pub panel_background: Option<String>,
    #[serde(rename = "surface.background", alias = "surface_background")]
    pub surface_background: Option<String>,
    #[serde(rename = "elevated_surface.background", alias = "elevated_surface_background")]
    pub elevated_surface_background: Option<String>,

    pub border: Option<String>,
    #[serde(rename = "border.variant", alias = "border_variant")]
    pub border_variant: Option<String>,

    #[serde(rename = "element.hover", alias = "element_hover")]
    pub element_hover: Option<String>,
    #[serde(rename = "element.selected", alias = "element_selected")]
    pub element_selected: Option<String>,
    #[serde(rename = "ghost_element.hover", alias = "ghost_element_hover")]
    pub ghost_element_hover: Option<String>,
    #[serde(rename = "ghost_element.selected", alias = "ghost_element_selected")]
    pub ghost_element_selected: Option<String>,

    pub text: Option<String>,
    #[serde(rename = "text.muted", alias = "text_muted")]
    pub text_muted: Option<String>,
    #[serde(rename = "text.placeholder", alias = "text_placeholder")]
    pub text_placeholder: Option<String>,
    #[serde(rename = "text.accent", alias = "text_accent")]
    pub text_accent: Option<String>,

    pub icon: Option<String>,
    #[serde(rename = "icon.muted", alias = "icon_muted")]
    pub icon_muted: Option<String>,
    #[serde(rename = "checkbox.border", alias = "checkbox_border")]
    pub checkbox_border: Option<String>,

    pub accent: Option<String>,
    #[serde(rename = "on_accent", alias = "text.on_accent")]
    pub on_accent: Option<String>,
    #[serde(rename = "icon.on_accent", alias = "icon_on_accent")]
    pub icon_on_accent: Option<String>,
    #[serde(rename = "text.selection", alias = "text_selection", alias = "element.selection_background")]
    pub text_selection: Option<String>,

    #[serde(default)]
    pub accents: Vec<Option<String>>,
    #[serde(default)]
    pub players: Vec<PlayerColorContent>,

    pub error: Option<String>,
    pub warning: Option<String>,
    pub info: Option<String>,
    pub success: Option<String>,
}

#[derive(Default)]
struct ThemeColorsRefinement {
    background: Option<Hsla>,
    sidebar_background: Option<Background>,
    sidebar_border: Option<Hsla>,
    sidebar_icon: Option<Hsla>,
    sidebar_icon_hover: Option<Hsla>,
    sidebar_icon_muted: Option<Hsla>,
    sidebar_icon_bottom: Option<Hsla>,
    avatar_background: Option<Hsla>,
    avatar_icon: Option<Hsla>,
    lists_background: Option<Background>,
    lists_border: Option<Hsla>,
    list_text: Option<Hsla>,
    list_text_muted: Option<Hsla>,
    list_count: Option<Hsla>,
    list_hover: Option<Hsla>,
    list_selected: Option<Hsla>,
    calendar_background: Option<Background>,
    matrix_background: Option<Background>,
    panel_background: Option<Hsla>,
    surface_background: Option<Hsla>,
    elevated_surface_background: Option<Hsla>,
    border: Option<Hsla>,
    border_variant: Option<Hsla>,
    element_hover: Option<Hsla>,
    element_selected: Option<Hsla>,
    ghost_element_hover: Option<Hsla>,
    ghost_element_selected: Option<Hsla>,
    text: Option<Hsla>,
    text_muted: Option<Hsla>,
    text_placeholder: Option<Hsla>,
    text_accent: Option<Hsla>,
    icon: Option<Hsla>,
    icon_muted: Option<Hsla>,
    checkbox_border: Option<Hsla>,
    accent: Option<Hsla>,
    on_accent: Option<Hsla>,
    icon_on_accent: Option<Hsla>,
    text_selection: Option<Hsla>,
}

macro_rules! apply_refinement_fields {
    ($target:expr, $refinement:expr, $($field:ident),+ $(,)?) => {
        $(
            if let Some(value) = $refinement.$field.clone() {
                $target.$field = value;
            }
        )+
    };
}

impl ThemeColorsRefinement {
    fn apply_to(self, colors: &mut ThemeColors) {
        apply_refinement_fields!(
            colors,
            self,
            background,
            sidebar_background,
            sidebar_border,
            sidebar_icon,
            sidebar_icon_hover,
            sidebar_icon_muted,
            sidebar_icon_bottom,
            avatar_background,
            avatar_icon,
            lists_background,
            lists_border,
            list_text,
            list_text_muted,
            list_count,
            list_hover,
            list_selected,
            calendar_background,
            matrix_background,
            panel_background,
            surface_background,
            elevated_surface_background,
            border,
            border_variant,
            element_hover,
            element_selected,
            ghost_element_hover,
            ghost_element_selected,
            text,
            text_muted,
            text_placeholder,
            text_accent,
            icon,
            icon_muted,
            checkbox_border,
            accent,
            on_accent,
            icon_on_accent,
            text_selection,
        );
    }
}

#[derive(Default)]
struct StatusColorsRefinement {
    error: Option<Hsla>,
    warning: Option<Hsla>,
    info: Option<Hsla>,
    success: Option<Hsla>,
}

impl StatusColorsRefinement {
    fn apply_to(self, status: &mut StatusColors) {
        let error_overridden = self.error.is_some();
        apply_refinement_fields!(status, self, error, warning, info, success);
        if error_overridden {
            status.on_error = foreground_on(&[status.error], 3.);
        }
    }
}

/// Strips `//` / `/* ... */` comments and trailing commas outside string literals, preserving newlines.
/// Strips an optional UTF-8 BOM, `//` and `/* ... */` JSONC comments, and valid trailing commas.
pub fn strip_jsonc(source: &str) -> Result<String> {
    let source = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let without_comments = strip_json_comments(source)?;
    Ok(strip_trailing_commas(&without_comments))
}

fn strip_json_comments(source: &str) -> Result<String> {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '\n' {
                    out.push('\n');
                } else if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                anyhow::bail!("unterminated block comment");
            }
        } else {
            out.push(ch);
        }
    }

    Ok(out)
}

fn strip_trailing_commas(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut prev_non_ws: Option<char> = None;

    for (idx, ch) in source.char_indices() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                prev_non_ws = Some('"');
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if ch == ',' {
            let rest = &source[idx + ch.len_utf8()..];
            let next_non_ws = rest.chars().find(|c| !c.is_whitespace());
            // Only strip trailing commas that follow an actual element/property, not `[,]` or `{,}`.
            let follows_container_open = matches!(prev_non_ws, None | Some('[' | '{' | ','));
            if !follows_container_open && matches!(next_non_ws, Some('}' | ']')) {
                continue;
            }
            out.push(ch);
            prev_non_ws = Some(ch);
        } else {
            out.push(ch);
            if !ch.is_whitespace() {
                prev_non_ws = Some(ch);
            }
        }
    }

    out
}

/// Returns `true` when `path` has a `.json` or `.jsonc` extension.
pub fn is_theme_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "json" | "jsonc"))
}

fn parse_opt_color(
    theme_name: &str,
    field: &str,
    value: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<Hsla> {
    let value = value?;
    match try_parse_color(value) {
        Ok(color) => Some(color),
        Err(err) => {
            warnings.push(format!("{theme_name}: invalid color for `{field}`: {err}"));
            None
        }
    }
}

fn parse_background(content: &BackgroundContent) -> Result<(Background, Option<[Hsla; 2]>)> {
    match content {
        BackgroundContent::Solid(color) => Ok((try_parse_color(color)?.into(), None)),
        BackgroundContent::Vertical([top, bottom]) => {
            let top = try_parse_color(top)?;
            let bottom = try_parse_color(bottom)?;
            Ok((presets::vertical_gradient(top, bottom), Some([top, bottom])))
        }
        BackgroundContent::Gradient { angle, stops: [top, bottom] } => {
            let top = try_parse_color(top)?;
            let bottom = try_parse_color(bottom)?;
            let bg = linear_gradient(*angle, linear_color_stop(top, 0.), linear_color_stop(bottom, 1.));
            Ok((bg, Some([top, bottom])))
        }
    }
}

fn parse_opt_background(
    theme_name: &str,
    field: &str,
    content: Option<&BackgroundContent>,
    warnings: &mut Vec<String>,
) -> (Option<Background>, Option<[Hsla; 2]>) {
    let Some(content) = content else {
        return (None, None);
    };
    match parse_background(content) {
        Ok((bg, stops)) => (Some(bg), stops),
        Err(err) => {
            warnings.push(format!("{theme_name}: invalid background for `{field}`: {err}"));
            (None, None)
        }
    }
}

fn parse_swatch(content: &SwatchContent) -> Result<[Hsla; 2]> {
    match content {
        SwatchContent::Solid(color) => {
            let color = try_parse_color(color)?;
            Ok([color, color])
        }
        SwatchContent::Gradient([top, bottom]) => {
            Ok([try_parse_color(top)?, try_parse_color(bottom)?])
        }
    }
}

/// Parses either a [`ThemeFamilyContent`] (`"themes": [...]`) or a single [`ThemeContent`].
pub fn parse_theme_family(source: &str) -> Result<ThemeFamilyContent> {
    let cleaned = strip_jsonc(source).context("invalid theme JSONC")?;
    let value: serde_json::Value = serde_json::from_str(&cleaned).context("invalid theme JSON")?;
    if value.as_object().is_some_and(|obj| obj.contains_key("themes")) {
        serde_json::from_value(value).context("invalid theme family schema")
    } else {
        let theme: ThemeContent =
            serde_json::from_value(value).context("invalid single-theme schema")?;
        Ok(ThemeFamilyContent {
            name: theme.name.clone(),
            author: theme.author.clone().unwrap_or_default(),
            themes: vec![theme],
        })
    }
}

/// Parses and refines all themes inside a JSON/JSONC theme document, returning themes and any non-fatal warnings.
pub fn load_theme_family_from_str(source: &str) -> Result<(Vec<Theme>, Vec<String>)> {
    let family = parse_theme_family(source)?;
    let family_author = (!family.author.trim().is_empty()).then_some(family.author.as_str());
    let mut themes = Vec::with_capacity(family.themes.len());
    let mut warnings = Vec::new();
    for content in &family.themes {
        let author = content
            .author
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or(family_author);
        let (theme, theme_warnings) = refine_theme(content, author);
        themes.push(theme);
        warnings.extend(theme_warnings);
    }
    Ok((themes, warnings))
}

/// Loads all `.json` and `.jsonc` theme files in `dir`, sorted by filename.
/// Missing directories return empty vectors; malformed files are reported in `errors` without aborting siblings.
pub fn load_themes_from_dir(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    if !dir.exists() {
        return (Vec::new(), Vec::new());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return (Vec::new(), vec![format!("cannot read {}: {err}", dir.display())]),
    };

    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file() && is_theme_file(path))
        .collect();
    paths.sort();

    let mut themes = Vec::new();
    let mut errors = Vec::new();

    for path in paths {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("theme.json");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) => {
                errors.push(format!("{file_name}: {err}"));
                continue;
            }
        };
        match load_theme_family_from_str(&source) {
            Ok((loaded, warnings)) => {
                themes.extend(loaded);
                for warning in warnings {
                    errors.push(format!("{file_name}: {warning}"));
                }
            }
            Err(err) => {
                errors.push(format!("{file_name}: {err:#}"));
            }
        }
    }

    (themes, errors)
}

fn refine_theme(content: &ThemeContent, author: Option<&str>) -> (Theme, Vec<String>) {
    let mut warnings = Vec::new();

    let trimmed_name = content.name.trim();
    let trimmed_id = content.id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let name: SharedString = if trimmed_name.is_empty() {
        warnings.push("theme is missing a non-empty `name`".to_owned());
        trimmed_id.unwrap_or("Custom Theme").to_owned().into()
    } else {
        trimmed_name.to_owned().into()
    };
    let id: SharedString = trimmed_id.map(str::to_owned).unwrap_or_else(|| name.to_string()).into();
    let label = name.as_ref();
    let dark = content.appearance.is_dark();

    let base_preset = match content.base.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(base_id) => match ThemePreset::find(base_id) {
            Some(preset) if preset.is_dark() == dark => preset,
            Some(_) => {
                warnings.push(format!("{label}: base preset `{base_id}` does not match theme appearance"));
                ThemePreset::resolve(dark, None)
            }
            None => {
                warnings.push(format!("{label}: unknown base preset `{base_id}`"));
                ThemePreset::resolve(dark, None)
            }
        },
        None => ThemePreset::resolve(dark, None),
    };

    let mut theme = Theme::from_preset(base_preset);
    let style = &content.style;

    let background_direct = parse_opt_color(label, "background", style.background.as_deref(), &mut warnings);
    let editor_background = parse_opt_color(label, "editor.background", style.editor_background.as_deref(), &mut warnings);
    let background_override = background_direct.or(editor_background);

    let (sidebar_background, sidebar_stops) =
        parse_opt_background(label, "sidebar.background", style.sidebar_background.as_ref(), &mut warnings);
    let (lists_background, _) =
        parse_opt_background(label, "lists.background", style.lists_background.as_ref(), &mut warnings);
    let (calendar_background, _) =
        parse_opt_background(label, "calendar.background", style.calendar_background.as_ref(), &mut warnings);
    let (matrix_background, _) =
        parse_opt_background(label, "matrix.background", style.matrix_background.as_ref(), &mut warnings);

    let accent_direct = parse_opt_color(label, "accent", style.accent.as_deref(), &mut warnings);
    let mut first_accent_array = None;
    for (idx, item) in style.accents.iter().enumerate() {
        let Some(raw) = item.as_deref() else { continue };
        match try_parse_color(raw) {
            Ok(color) => {
                if first_accent_array.is_none() {
                    first_accent_array = Some(color);
                }
            }
            Err(err) => warnings.push(format!("{label}: invalid color for `accents[{idx}]`: {err}")),
        }
    }

    let mut first_player_selection = None;
    let mut first_player_background = None;
    if let Some(player) = style.players.first() {
        first_player_background =
            parse_opt_color(label, "players[0].background", player.background.as_deref(), &mut warnings);
        first_player_selection =
            parse_opt_color(label, "players[0].selection", player.selection.as_deref(), &mut warnings);
    }

    let text_override = parse_opt_color(label, "text", style.text.as_deref(), &mut warnings);
    let text_accent_override = parse_opt_color(label, "text.accent", style.text_accent.as_deref(), &mut warnings);
    let panel_override = parse_opt_color(label, "panel.background", style.panel_background.as_deref(), &mut warnings);
    let surface_override = parse_opt_color(label, "surface.background", style.surface_background.as_deref(), &mut warnings);
    let elevated_override = parse_opt_color(
        label,
        "elevated_surface.background",
        style.elevated_surface_background.as_deref(),
        &mut warnings,
    );
    let text_selection_explicit = parse_opt_color(label, "text.selection", style.text_selection.as_deref(), &mut warnings);

    let preserve_base_gradient = content.base.is_some() && base_preset.gradient.is_some();

    let accent_override = accent_direct.or(first_accent_array).or_else(|| {
        if !preserve_base_gradient {
            text_accent_override.or(first_player_background)
        } else {
            None
        }
    });

    let refinement = ThemeColorsRefinement {
        background: background_override,
        sidebar_background,
        sidebar_border: parse_opt_color(label, "sidebar.border", style.sidebar_border.as_deref(), &mut warnings),
        sidebar_icon: parse_opt_color(label, "sidebar.icon", style.sidebar_icon.as_deref(), &mut warnings),
        sidebar_icon_hover: parse_opt_color(label, "sidebar.icon_hover", style.sidebar_icon_hover.as_deref(), &mut warnings),
        sidebar_icon_muted: parse_opt_color(label, "sidebar.icon_muted", style.sidebar_icon_muted.as_deref(), &mut warnings),
        sidebar_icon_bottom: parse_opt_color(label, "sidebar.icon_bottom", style.sidebar_icon_bottom.as_deref(), &mut warnings),
        avatar_background: parse_opt_color(label, "avatar.background", style.avatar_background.as_deref(), &mut warnings),
        avatar_icon: parse_opt_color(label, "avatar.icon", style.avatar_icon.as_deref(), &mut warnings),
        lists_background,
        lists_border: parse_opt_color(label, "lists.border", style.lists_border.as_deref(), &mut warnings),
        list_text: parse_opt_color(label, "list.text", style.list_text.as_deref(), &mut warnings),
        list_text_muted: parse_opt_color(label, "list.text_muted", style.list_text_muted.as_deref(), &mut warnings),
        list_count: parse_opt_color(label, "list.count", style.list_count.as_deref(), &mut warnings),
        list_hover: parse_opt_color(label, "list.hover", style.list_hover.as_deref(), &mut warnings),
        list_selected: parse_opt_color(label, "list.selected", style.list_selected.as_deref(), &mut warnings),
        calendar_background,
        matrix_background,
        panel_background: panel_override,
        surface_background: surface_override,
        elevated_surface_background: elevated_override,
        border: parse_opt_color(label, "border", style.border.as_deref(), &mut warnings),
        border_variant: parse_opt_color(label, "border.variant", style.border_variant.as_deref(), &mut warnings),
        element_hover: parse_opt_color(label, "element.hover", style.element_hover.as_deref(), &mut warnings),
        element_selected: parse_opt_color(label, "element.selected", style.element_selected.as_deref(), &mut warnings),
        ghost_element_hover: parse_opt_color(label, "ghost_element.hover", style.ghost_element_hover.as_deref(), &mut warnings),
        ghost_element_selected: parse_opt_color(label, "ghost_element.selected", style.ghost_element_selected.as_deref(), &mut warnings),
        text: text_override,
        text_muted: parse_opt_color(label, "text.muted", style.text_muted.as_deref(), &mut warnings),
        text_placeholder: parse_opt_color(label, "text.placeholder", style.text_placeholder.as_deref(), &mut warnings),
        text_accent: text_accent_override,
        icon: parse_opt_color(label, "icon", style.icon.as_deref(), &mut warnings),
        icon_muted: parse_opt_color(label, "icon.muted", style.icon_muted.as_deref(), &mut warnings),
        checkbox_border: parse_opt_color(label, "checkbox.border", style.checkbox_border.as_deref(), &mut warnings),
        accent: accent_override,
        on_accent: parse_opt_color(label, "on_accent", style.on_accent.as_deref(), &mut warnings),
        icon_on_accent: parse_opt_color(label, "icon.on_accent", style.icon_on_accent.as_deref(), &mut warnings),
        text_selection: text_selection_explicit.or(first_player_selection),
    };

    let status_refinement = StatusColorsRefinement {
        error: parse_opt_color(label, "error", style.error.as_deref(), &mut warnings),
        warning: parse_opt_color(label, "warning", style.warning.as_deref(), &mut warnings),
        info: parse_opt_color(label, "info", style.info.as_deref(), &mut warnings),
        success: parse_opt_color(label, "success", style.success.as_deref(), &mut warnings),
    };

    let default_ink: Hsla = rgb(if dark { 0xffffff } else { 0x191919 }).into();
    let ink = text_override.map(|c| c.opacity(1.0)).unwrap_or(default_ink);
    let background = background_override.unwrap_or(theme.colors.background);
    let accent = accent_override.unwrap_or(theme.colors.accent);

    if !preserve_base_gradient {
        if background_override.is_some()
            || panel_override.is_some()
            || surface_override.is_some()
            || elevated_override.is_some()
            || text_override.is_some()
        {
            let panel = panel_override.or(surface_override).unwrap_or_else(|| {
                if background_override.is_some() {
                    background.blend(ink.opacity(if dark { 0.05 } else { 0.03 }))
                } else {
                    theme.colors.panel_background
                }
            });
            let surface = surface_override.unwrap_or_else(|| {
                if panel_override.is_some() || background_override.is_some() {
                    if dark { panel } else { background.blend(ink.opacity(0.05)) }
                } else {
                    theme.colors.surface_background
                }
            });
            let elevated = elevated_override.unwrap_or_else(|| {
                if panel_override.is_some() || background_override.is_some() {
                    if dark { panel } else { background }
                } else {
                    theme.colors.elevated_surface_background
                }
            });

            let border_variant = background.blend(ink.opacity(if dark { 0.07 } else { 0.08 }));
            let text_muted = background.blend(ink.opacity(if dark { 0.60 } else { 0.62 }));
            let hover = ink.opacity(if dark { 0.06 } else { 0.04 });

            theme.colors.background = background;
            theme.colors.panel_background = panel;
            theme.colors.surface_background = surface;
            theme.colors.elevated_surface_background = elevated;
            theme.colors.sidebar_background = panel.into();
            theme.colors.lists_background = background.into();
            theme.colors.calendar_background = background.into();
            theme.colors.matrix_background = panel.into();
            theme.colors.border = background.blend(ink.opacity(if dark { 0.14 } else { 0.15 }));
            theme.colors.border_variant = border_variant;
            theme.colors.sidebar_border = border_variant;
            theme.colors.lists_border = border_variant;
            theme.colors.text = if dark { background.blend(ink.opacity(0.90)) } else { ink };
            theme.colors.text_muted = text_muted;
            theme.colors.text_placeholder = text_muted;
            theme.colors.icon = background.blend(ink.opacity(0.85));
            theme.colors.icon_muted = text_muted;
            theme.colors.checkbox_border = background.blend(ink.opacity(0.48));
            theme.colors.sidebar_icon = if dark { ink } else { accent };
            theme.colors.sidebar_icon_hover = ink.opacity(0.60);
            theme.colors.sidebar_icon_muted = ink.opacity(0.40);
            theme.colors.sidebar_icon_bottom = ink.opacity(0.40);
            theme.colors.avatar_background =
                panel.blend(ink.opacity(if dark { 0.08 } else { 0.07 }));
            theme.colors.avatar_icon = panel.blend(ink.opacity(0.64));
            theme.colors.list_text = theme.colors.text;
            theme.colors.list_text_muted = ink.opacity(if dark { 0.50 } else { 0.55 });
            theme.colors.list_count = ink.opacity(if dark { 0.45 } else { 0.50 });
            theme.colors.element_hover = surface.blend(hover);
            theme.colors.ghost_element_hover = hover;
            theme.colors.ghost_element_selected = ink.opacity(if dark { 0.08 } else { 0.06 });
            theme.colors.list_hover = hover;
            theme.colors.list_selected = ink.opacity(if dark { 0.08 } else { 0.06 });
        }

        if accent_override.is_some() || background_override.is_some() || text_override.is_some() {
            let on_accent = foreground_on(&[accent], 3.8);
            theme.colors.accent = accent;
            theme.colors.text_accent = readable_accent(accent, background, ink);
            theme.colors.on_accent = on_accent;
            theme.colors.icon_on_accent = on_accent;
            theme.colors.element_selected = background.blend(accent.opacity(if dark { 0.22 } else { 0.13 }));
            theme.colors.text_selection = accent.opacity(if dark { 0.34 } else { 0.25 });
            if !dark {
                theme.colors.sidebar_icon = accent;
            }
        }
    } else {
        if background_override.is_some() {
            theme.colors.background = background;
        }
        if accent_override.is_some() || background_override.is_some() || text_override.is_some() {
            theme.colors.text_accent = readable_accent(accent, theme.colors.background, ink);
        }
        if accent_override.is_some() {
            let on_accent = foreground_on(&[accent], 3.8);
            theme.colors.accent = accent;
            theme.colors.on_accent = on_accent;
            theme.colors.icon_on_accent = on_accent;
            theme.colors.element_selected =
                theme.colors.background.blend(accent.opacity(if dark { 0.22 } else { 0.13 }));
            theme.colors.text_selection = accent.opacity(if dark { 0.34 } else { 0.25 });
        }
    }

    // Apply explicit overrides last so user-provided values always win over derived colors.
    refinement.apply_to(&mut theme.colors);
    status_refinement.apply_to(&mut theme.status);

    let swatch_override = content.swatch.as_ref().and_then(|s| match parse_swatch(s) {
        Ok(swatch) => Some(swatch),
        Err(err) => {
            warnings.push(format!("{label}: invalid swatch: {err}"));
            None
        }
    });

    let swatch = swatch_override
        .or(sidebar_stops)
        .unwrap_or_else(|| {
            if preserve_base_gradient && accent_override.is_none() && background_override.is_none() {
                base_preset.preview_colors()
            } else {
                [theme.colors.accent, theme.colors.background]
            }
        });

    theme.id = id;
    theme.name = name;
    theme.author = author.map(str::trim).filter(|s| !s.is_empty()).map(SharedString::from);
    theme.appearance = content.appearance;
    theme.is_custom = true;
    theme.swatch = swatch;

    (theme, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contrast;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn example_theme_template_parses_cleanly_with_legible_accents() {
        let (themes, warnings) = load_theme_family_from_str(EXAMPLE_THEME_TEMPLATE).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(themes.len(), 2);
        for theme in &themes {
            assert!(theme.is_custom());
            assert!(contrast(theme.colors().text_accent, theme.colors().background) >= 4.49, "{}", theme.id());
            assert!(contrast(theme.colors().on_accent, theme.colors().accent) >= 3.8, "{}", theme.id());
        }
        assert_eq!(themes[0].id(), "fern-dusk");
        assert!(themes[0].is_dark());
        assert_eq!(themes[1].id(), "fern-paper");
        assert!(themes[1].is_light());
        let paper = &themes[1];
        assert!(contrast(paper.colors().text, paper.colors().background) >= 7.0);
        assert!(contrast(paper.colors().text_muted, paper.colors().background) >= 4.5);
        assert!(contrast(paper.colors().list_text_muted, try_parse_color("#f5f0e7").unwrap()) >= 4.5);
    }

    #[test]
    fn non_gradient_base_re_derives_sidebar_borders_and_muted_colors() {
        let json = r##"{
            "name": "Warm Cream",
            "appearance": "light",
            "base": "grey",
            "style": {
                "background": "#faf7f2",
                "panel.background": "#efe8dc",
                "text": "#2b241f",
                "accent": "#c05d3b"
            }
        }"##;

        let default_light = Theme::light();
        let (themes, warnings) = load_theme_family_from_str(json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let theme = &themes[0];
        assert_eq!(
            theme.colors().sidebar_background,
            Background::from(try_parse_color("#efe8dc").unwrap())
        );
        assert_ne!(theme.colors().avatar_background, default_light.colors().avatar_background);
        assert_ne!(theme.colors().border_variant, default_light.colors().border_variant);
        assert_ne!(theme.colors().text_muted, default_light.colors().text_muted);
        assert_eq!(theme.colors().list_text, try_parse_color("#2b241f").unwrap());
    }

    #[test]
    fn parses_theme_family_with_jsonc_and_ignores_unknown_keys() {
        let family_json = r##"{
            "$schema": "https://example.com/schema/themes.json", // URL with // must stay intact
            "name": "Catppuccin",
            "author": "Catppuccin Org",
            "themes": [
                {
                    "name": "Catppuccin Mocha",
                    "appearance": "dark",
                    /* block comment with trailing commas */
                    "style": {
                        "background": "#1e1e2eff",
                        "editor.background": "#1e1e2e",
                        "panel.background": "#181825",
                        "surface.background": "#313244",
                        "text": "#cdd6f4",
                        "text.muted": "#a6adc8",
                        "border.variant": "#45475a",
                        "accents": ["#89b4fa", "#f5c2e7"],
                        "players": [
                            {
                                "cursor": "#f5e0dc",
                                "background": "#89b4fa",
                                "selection": "#89b4fa40",
                            },
                        ],
                        "error": "#f38ba8",
                        "syntax": {
                            "comment": { "color": "#6c7086", "font_style": "italic" },
                        },
                        "terminal.ansi.red": "#f38ba8",
                    },
                },
            ],
        }"##;

        let (themes, warnings) = load_theme_family_from_str(family_json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(themes.len(), 1);
        let theme = &themes[0];
        assert_eq!(theme.id(), "Catppuccin Mocha");
        assert_eq!(theme.name(), "Catppuccin Mocha");
        assert_eq!(theme.author(), Some("Catppuccin Org"));
        assert!(theme.is_dark());
        assert_eq!(theme.colors().background, try_parse_color("#1e1e2eff").unwrap());
        assert_eq!(theme.colors().panel_background, try_parse_color("#181825").unwrap());
        assert_eq!(theme.colors().text, try_parse_color("#cdd6f4").unwrap());
        assert_eq!(theme.colors().text_muted, try_parse_color("#a6adc8").unwrap());
        assert_eq!(theme.colors().border_variant, try_parse_color("#45475a").unwrap());
        assert_eq!(theme.colors().accent, try_parse_color("#89b4fa").unwrap());
        assert_eq!(theme.colors().text_selection, try_parse_color("#89b4fa40").unwrap());
        assert_eq!(theme.status().error, try_parse_color("#f38ba8").unwrap());
    }

    #[test]
    fn parses_single_theme_minimal_and_derives_secondary_colors() {
        let json = r##"{
            "name": "Nord Minimal",
            "appearance": "dark",
            "style": {
                "background": "#2e3440",
                "panel.background": "#242933",
                "text": "#eceff4",
                "accent": "#88c0d0"
            }
        }"##;

        let (themes, warnings) = load_theme_family_from_str(json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let theme = &themes[0];
        assert_eq!(theme.colors().background, try_parse_color("#2e3440").unwrap());
        assert_eq!(theme.colors().sidebar_background, Background::from(try_parse_color("#242933").unwrap()));
        assert_eq!(theme.colors().lists_background, Background::from(try_parse_color("#2e3440").unwrap()));
        assert!(contrast(theme.colors().on_accent, theme.colors().accent) >= 3.8);
        assert!(contrast(theme.colors().text_accent, theme.colors().background) >= 4.49);
        assert_ne!(theme.colors().border_variant, Theme::dark().colors().border_variant);
        assert_ne!(theme.colors().text_muted, Theme::dark().colors().text_muted);
    }

    #[test]
    fn base_preset_inheritance_preserves_gradients_and_applies_overrides() {
        let json = r##"{
            "name": "Matcha Mint",
            "appearance": "light",
            "base": "matcha",
            "style": {
                "accent": "#2b7a5b"
            }
        }"##;

        let base_matcha = Theme::from_preset(ThemePreset::resolve(false, Some("matcha")));
        let (themes, warnings) = load_theme_family_from_str(json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let custom = &themes[0];
        assert_eq!(custom.colors().sidebar_background, base_matcha.colors().sidebar_background);
        assert_eq!(custom.colors().lists_background, base_matcha.colors().lists_background);
        assert_eq!(custom.colors().accent, try_parse_color("#2b7a5b").unwrap());
    }

    #[test]
    fn gradients_and_invalid_colors_report_warnings_without_dropping_theme() {
        let json = r##"{
            "name": "Bad Color Test",
            "appearance": "dark",
            "style": {
                "accent": "not-a-hex-color",
                "sidebar.background": { "angle": 90, "stops": ["#112233", "#445566"] },
                "lists.background": ["#101018", "#181824"],
                "error": "#ff5555"
            }
        }"##;

        let (themes, warnings) = load_theme_family_from_str(json).unwrap();
        assert_eq!(themes.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("accent"), "{warnings:?}");
        assert_eq!(themes[0].status().error, try_parse_color("#ff5555").unwrap());
        assert_eq!(themes[0].colors().accent, Theme::dark().colors().accent);
        assert_ne!(themes[0].colors().sidebar_background, Theme::dark().colors().sidebar_background);
    }

    #[test]
    fn load_themes_from_dir_handles_missing_valid_and_broken_files() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("fern-theme-dir-test-{unique}"));
        let (empty_themes, empty_errors) = load_themes_from_dir(&dir);
        assert!(empty_themes.is_empty());
        assert!(empty_errors.is_empty());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("01-good.jsonc"), EXAMPLE_THEME_TEMPLATE).unwrap();
        std::fs::write(dir.join("02-broken.json"), "{ invalid json").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let (themes, errors) = load_themes_from_dir(&dir);
        assert_eq!(themes.len(), 2);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("02-broken.json:"), "{errors:?}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
