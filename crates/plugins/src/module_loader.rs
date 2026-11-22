//! Sandboxed QuickJS ES module resolver and loader (`plugin_dir` + virtual `fern` / `fern:ui`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{Ctx, Error, Module, Result};

const MAX_MODULE_BYTES: u64 = 2 * 1024 * 1024;

pub const VIRTUAL_FERN_MODULE: &str = "fern";
pub const VIRTUAL_FERN_UI_MODULE: &str = "fern:ui";

const FERN_MODULE_SOURCE: &str = r#"
const f = globalThis.__fern;
export const tasks = f.tasks;
export const lists = f.lists;
export const tags = f.tags;
export const habits = f.habits;
export const focus = f.focus;
export const countdowns = f.countdowns;
export const batch = f.batch;
export const storage = f.storage;
export const meta = f.meta;
export const settings = f.settings;
export const http = f.http;
export const events = f.events;
export const hooks = f.hooks;
export const timers = f.timers;
export const commands = f.commands;
export const search = f.search;
export const services = f.services;
export const views = f.views;
export const ui = f.ui;
export const system = f.system;
export default f;
"#;

const FERN_UI_MODULE_SOURCE: &str = r#"
function wrapHandler(fn) {
    if (typeof fn === "function") {
        return globalThis.__fern_reg_handler(fn);
    }
    return fn;
}

function normalizeChildren(arg1, arg2) {
    if (Array.isArray(arg1)) {
        return [{}, arg1];
    }
    if (arg1 && typeof arg1 === "object" && !Array.isArray(arg1)) {
        const props = { ...arg1 };
        const children = Array.isArray(arg2)
            ? arg2
            : Array.isArray(props.children)
              ? props.children
              : [];
        delete props.children;
        return [props, children];
    }
    return [{}, Array.isArray(arg2) ? arg2 : []];
}

function normalizeNode(node) {
    if (!node || typeof node !== "object") {
        return node;
    }
    const out = { ...node };
    for (const key of ["onClick", "on_click", "onChange", "on_change", "onSubmit", "on_submit", "onAction", "on_action"]) {
        if (typeof out[key] === "function") {
            out[key] = wrapHandler(out[key]);
        }
    }
    if (Array.isArray(out.children)) {
        out.children = out.children.map(normalizeNode);
    }
    if (out.child && typeof out.child === "object") {
        out.child = normalizeNode(out.child);
    }
    return out;
}

export function vstack(propsOrChildren, maybeChildren) {
    const [props, children] = normalizeChildren(propsOrChildren, maybeChildren);
    return normalizeNode({ type: "vstack", ...props, children });
}

export function hstack(propsOrChildren, maybeChildren) {
    const [props, children] = normalizeChildren(propsOrChildren, maybeChildren);
    return normalizeNode({ type: "hstack", ...props, children });
}

export function grid(propsOrChildren, maybeChildren) {
    const [props, children] = normalizeChildren(propsOrChildren, maybeChildren);
    return normalizeNode({ type: "grid", ...props, children });
}

export function scroll(props, maybeChildren) {
    const [norm, children] = normalizeChildren(props, maybeChildren);
    return normalizeNode({ type: "scroll", ...norm, children });
}

export function card(propsOrChildren, maybeChildren) {
    const [props, children] = normalizeChildren(propsOrChildren, maybeChildren);
    return normalizeNode({ type: "card", ...props, children });
}

export function section(propsOrTitle, maybeChildren) {
    if (typeof propsOrTitle === "string") {
        const children = Array.isArray(maybeChildren) ? maybeChildren : [];
        return normalizeNode({ type: "section", title: propsOrTitle, children });
    }
    const [props, children] = normalizeChildren(propsOrTitle, maybeChildren);
    return normalizeNode({ type: "section", ...props, children });
}

export function divider() {
    return { type: "divider" };
}

export function spacer(size) {
    if (size && typeof size === "object") {
        return { type: "spacer", ...size };
    }
    return { type: "spacer", size };
}

export function heading(propsOrText, level) {
    if (typeof propsOrText === "string") {
        return { type: "heading", text: propsOrText, level: level ?? 2 };
    }
    return { type: "heading", ...propsOrText };
}

export function text(propsOrText) {
    if (typeof propsOrText === "string") {
        return { type: "text", text: propsOrText };
    }
    return { type: "text", ...propsOrText };
}

export function markdown(propsOrContent) {
    if (typeof propsOrContent === "string") {
        return { type: "markdown", content: propsOrContent };
    }
    return { type: "markdown", ...propsOrContent };
}

export function keyValue(props) {
    return { type: "key_value", ...props };
}

export function badge(propsOrLabel) {
    if (typeof propsOrLabel === "string") {
        return { type: "badge", label: propsOrLabel };
    }
    return { type: "badge", ...propsOrLabel };
}

export function progress(propsOrValue) {
    if (typeof propsOrValue === "number") {
        return { type: "progress", value: propsOrValue };
    }
    return { type: "progress", ...propsOrValue };
}

export function donut(propsOrValue) {
    if (typeof propsOrValue === "number") {
        return { type: "donut", value: propsOrValue };
    }
    return { type: "donut", ...propsOrValue };
}

export function emptyState(props) {
    return normalizeNode({ type: "empty_state", ...props });
}

export function svg(props) {
    return { type: "svg", ...props };
}

export function button(props) {
    return normalizeNode({ type: "button", ...props });
}

export function iconButton(props) {
    return normalizeNode({ type: "icon_button", ...props });
}

export function input(props) {
    return normalizeNode({ type: "input", ...props });
}

export function textArea(props) {
    return normalizeNode({ type: "text_area", ...props });
}

export function select(props) {
    return normalizeNode({ type: "select", ...props });
}

export function checkbox(props) {
    return normalizeNode({ type: "checkbox", ...props });
}

export function toggle(props) {
    return normalizeNode({ type: "switch", ...props });
}

export { toggle as switchControl };

export function segmentedTabs(props) {
    return normalizeNode({ type: "segmented_tabs", ...props });
}

export function listItem(props) {
    return normalizeNode({ type: "list_item", ...props });
}

export function registerHandler(fn) {
    return wrapHandler(fn);
}

export default {
    vstack,
    hstack,
    grid,
    scroll,
    card,
    section,
    divider,
    spacer,
    heading,
    text,
    markdown,
    keyValue,
    badge,
    progress,
    donut,
    emptyState,
    svg,
    button,
    iconButton,
    input,
    textArea,
    select,
    checkbox,
    switch: toggle,
    toggle,
    segmentedTabs,
    listItem,
    registerHandler,
};
"#;

/// Sandboxed module resolver allowing only `fern`, `fern:ui`, and relative JS paths strictly inside `plugin_dir`.
pub struct SandboxedResolver {
    canonical_plugin_dir: PathBuf,
}

impl SandboxedResolver {
    pub fn new(canonical_plugin_dir: PathBuf) -> Self {
        Self {
            canonical_plugin_dir,
        }
    }
}

impl Resolver for SandboxedResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        if name == VIRTUAL_FERN_MODULE || name == VIRTUAL_FERN_UI_MODULE {
            return Ok(name.to_owned());
        }

        // Only explicit relative paths (`./` or `../`) are permitted; bare imports like "fs" are blocked.
        let is_relative = name.starts_with("./")
            || name.starts_with("../")
            || name.starts_with(".\\")
            || name.starts_with("..\\");
        if !is_relative {
            return Err(Error::new_resolving_message(
                base,
                name,
                "bare module specifiers and absolute imports are forbidden; use ./relative.js or 'fern'/'fern:ui'",
            ));
        }

        let base_dir = resolve_base_dir(&self.canonical_plugin_dir, base);
        let rel_path = Path::new(name);
        if rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        {
            return Err(Error::new_resolving_message(
                base,
                name,
                "absolute paths are forbidden",
            ));
        }

        let candidate = base_dir.join(rel_path);
        let resolved_path = resolve_js_candidate(&candidate).ok_or_else(|| {
            Error::new_resolving_message(base, name, "module not found inside plugin directory")
        })?;

        let canon = resolved_path.canonicalize().map_err(|err| {
            Error::new_resolving_message(base, name, format!("cannot resolve module: {err}"))
        })?;
        if !canon.starts_with(&self.canonical_plugin_dir) {
            return Err(Error::new_resolving_message(
                base,
                name,
                "path traversal or symlink escape outside plugin directory is forbidden",
            ));
        }
        if !canon.is_file() {
            return Err(Error::new_resolving_message(
                base,
                name,
                "module target is not a regular file",
            ));
        }
        Ok(canon.to_string_lossy().into_owned())
    }
}

/// Sandboxed module loader serving `fern`, `fern:ui`, and canonical files inside `plugin_dir`.
pub struct SandboxedLoader {
    canonical_plugin_dir: PathBuf,
}

impl SandboxedLoader {
    pub fn new(canonical_plugin_dir: PathBuf) -> Self {
        Self {
            canonical_plugin_dir,
        }
    }
}

impl Loader for SandboxedLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js>> {
        if name == VIRTUAL_FERN_MODULE {
            return Module::declare(ctx.clone(), name, FERN_MODULE_SOURCE);
        }
        if name == VIRTUAL_FERN_UI_MODULE {
            return Module::declare(ctx.clone(), name, FERN_UI_MODULE_SOURCE);
        }

        let canon = Path::new(name).canonicalize().map_err(|err| {
            Error::new_loading_message(name, format!("cannot canonicalize module: {err}"))
        })?;
        if !canon.starts_with(&self.canonical_plugin_dir) {
            return Err(Error::new_loading_message(
                name,
                "module path escapes plugin directory",
            ));
        }
        let meta = fs::metadata(&canon).map_err(|err| {
            Error::new_loading_message(name, format!("cannot stat module: {err}"))
        })?;
        if !meta.is_file() || meta.len() > MAX_MODULE_BYTES {
            return Err(Error::new_loading_message(
                name,
                "module is not a valid file or exceeds 2 MiB size cap",
            ));
        }
        let source = fs::read_to_string(&canon).map_err(|err| {
            Error::new_loading_message(name, format!("cannot read module source: {err}"))
        })?;
        Module::declare(ctx.clone(), name, source)
    }
}

fn resolve_base_dir(canonical_plugin_dir: &Path, base: &str) -> PathBuf {
    let base_path = Path::new(base);
    if base_path.is_absolute()
        && let Ok(canon) = base_path.canonicalize()
        && canon.starts_with(canonical_plugin_dir)
    {
        return canon
            .parent()
            .unwrap_or(canonical_plugin_dir)
            .to_path_buf();
    }
    let joined = canonical_plugin_dir.join(base_path);
    if let Ok(canon) = joined.canonicalize()
        && canon.starts_with(canonical_plugin_dir)
    {
        return canon
            .parent()
            .unwrap_or(canonical_plugin_dir)
            .to_path_buf();
    }
    canonical_plugin_dir.to_path_buf()
}

fn resolve_js_candidate(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    if candidate.extension().is_none() {
        let with_js = candidate.with_extension("js");
        if with_js.is_file() {
            return Some(with_js);
        }
        let index_js = candidate.join("index.js");
        if index_js.is_file() {
            return Some(index_js);
        }
    }
    None
}
