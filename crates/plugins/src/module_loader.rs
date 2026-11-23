//! Sandboxed QuickJS ES module resolver and loader (`plugin_dir` + virtual `fern` / `fern:ui`).

use std::fs::File;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{Ctx, Error, Module, Result};

const MAX_MODULE_BYTES: u64 = 2 * 1024 * 1024;
const UNIFORM_RESOLVE_ERROR: &str = "module not found or forbidden inside plugin directory";

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
export const oauth = f.oauth;
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

// Keep JS callback functions untouched on UI nodes so the Rust host (`js_value_to_ui_node`)
// registers them into the exact active render/modal scope (fixing `openModal`, `defineView`,
// and `useMemo` across re-renders without leaking handlers into `__default__`).
function normalizeNode(node) {
    if (!node || typeof node !== "object") {
        return node;
    }
    const out = { ...node };
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
    if (typeof fn === "function") {
        return globalThis.__fern_reg_handler(fn);
    }
    return fn;
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

        let reject = || Error::new_resolving_message(base, name, UNIFORM_RESOLVE_ERROR);

        if name.contains('\0') {
            return Err(reject());
        }

        // Only explicit relative paths (`./` or `../`) are permitted; bare imports like "fs" are blocked.
        let is_relative = name.starts_with("./")
            || name.starts_with("../")
            || name.starts_with(".\\")
            || name.starts_with("..\\");
        if !is_relative {
            return Err(reject());
        }

        let base_dir = resolve_base_dir(&self.canonical_plugin_dir, base);
        let Some(lexical) = lexical_join_within_root(&self.canonical_plugin_dir, &base_dir, Path::new(name)) else {
            return Err(reject());
        };

        let Some(resolved_path) = resolve_js_candidate(&self.canonical_plugin_dir, &lexical) else {
            return Err(reject());
        };

        let Ok(canon) = resolved_path.canonicalize() else {
            return Err(reject());
        };
        if !canon.starts_with(&self.canonical_plugin_dir)
            || !has_allowed_js_extension(&canon)
            || !canon.is_file()
        {
            return Err(reject());
        }
        Ok(canon.to_string_lossy().into_owned())
    }
}

/// Sandboxed module loader serving `fern`, `fern:ui`, and canonical `.js`/`.mjs` files inside `plugin_dir`.
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

        let reject = || Error::new_loading_message(name, "cannot load module");

        let Ok(canon) = Path::new(name).canonicalize() else {
            return Err(reject());
        };
        if !canon.starts_with(&self.canonical_plugin_dir) || !has_allowed_js_extension(&canon) {
            return Err(reject());
        }
        let Ok(file) = File::open(&canon) else {
            return Err(reject());
        };
        let Ok(meta) = file.metadata() else {
            return Err(reject());
        };
        if !meta.is_file() {
            return Err(reject());
        }
        let mut bytes = Vec::new();
        if file
            .take(MAX_MODULE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_MODULE_BYTES
        {
            return Err(reject());
        }
        let Ok(source) = String::from_utf8(bytes) else {
            return Err(reject());
        };
        Module::declare(ctx.clone(), name, source)
    }
}

fn has_allowed_js_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("js") || ext.eq_ignore_ascii_case("mjs"))
}

fn lexical_join_within_root(root: &Path, base_dir: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.is_absolute() || !base_dir.starts_with(root) {
        return None;
    }
    let mut out = base_dir.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() || !out.starts_with(root) {
                    return None;
                }
            }
            Component::Normal(seg) => {
                out.push(seg);
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    out.starts_with(root).then_some(out)
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
    if let Some(lexical) = lexical_join_within_root(canonical_plugin_dir, canonical_plugin_dir, base_path)
        && let Ok(canon) = lexical.canonicalize()
        && canon.starts_with(canonical_plugin_dir)
    {
        return canon
            .parent()
            .unwrap_or(canonical_plugin_dir)
            .to_path_buf();
    }
    canonical_plugin_dir.to_path_buf()
}

fn resolve_js_candidate(canonical_plugin_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    if !candidate.starts_with(canonical_plugin_dir) {
        return None;
    }
    if candidate.extension().is_some() {
        if has_allowed_js_extension(candidate) && candidate.is_file() {
            return Some(candidate.to_path_buf());
        }
        return None;
    }
    let with_js = candidate.with_extension("js");
    if with_js.starts_with(canonical_plugin_dir) && with_js.is_file() {
        return Some(with_js);
    }
    let with_mjs = candidate.with_extension("mjs");
    if with_mjs.starts_with(canonical_plugin_dir) && with_mjs.is_file() {
        return Some(with_mjs);
    }
    let index_js = candidate.join("index.js");
    if index_js.starts_with(canonical_plugin_dir) && index_js.is_file() {
        return Some(index_js);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rquickjs::{Context, Runtime};

    use super::*;

    #[test]
    fn resolver_returns_uniform_error_without_existence_oracle_and_restricts_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let canon_plugin = plugin_dir.canonicalize().unwrap();

        // Create an existing file OUTSIDE plugin_dir and a non-JS file INSIDE plugin_dir.
        fs::write(tmp.path().join("outside_exists.js"), "export const x = 1;").unwrap();
        fs::write(plugin_dir.join("helper.js"), "export const ok = 42;").unwrap();
        fs::write(plugin_dir.join("manifest.json"), r#"{"id":"x"}"#).unwrap();

        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        let mut resolver = SandboxedResolver::new(canon_plugin.clone());

        ctx.with(|ctx| {
            // Valid JS import inside plugin_dir succeeds (explicit and extensionless).
            let ok1 = resolver.resolve(&ctx, "index.js", "./helper.js", None).unwrap();
            let ok2 = resolver.resolve(&ctx, "index.js", "./helper", None).unwrap();
            assert_eq!(ok1, ok2);

            // Existing outside file, nonexistent outside file, nonexistent inside file, and
            // non-JS file inside plugin_dir must all return the exact same uniform message!
            let err_outside_exists = resolver
                .resolve(&ctx, "index.js", "../outside_exists.js", None)
                .unwrap_err()
                .to_string();
            let err_outside_missing = resolver
                .resolve(&ctx, "index.js", "../outside_missing.js", None)
                .unwrap_err()
                .to_string();
            let err_inside_missing = resolver
                .resolve(&ctx, "index.js", "./inside_missing.js", None)
                .unwrap_err()
                .to_string();
            let err_non_js = resolver
                .resolve(&ctx, "index.js", "./manifest.json", None)
                .unwrap_err()
                .to_string();

            for err in [
                &err_outside_exists,
                &err_outside_missing,
                &err_inside_missing,
                &err_non_js,
            ] {
                assert!(
                    err.contains(UNIFORM_RESOLVE_ERROR),
                    "expected uniform error message, got: {err}"
                );
            }
            // Strip the specifier name itself and compare — error details must be indistinguishable.
            assert_eq!(
                err_outside_exists.replace("../outside_exists.js", "<X>"),
                err_outside_missing.replace("../outside_missing.js", "<X>")
            );
            assert_eq!(
                err_outside_exists.replace("../outside_exists.js", "<X>"),
                err_inside_missing.replace("./inside_missing.js", "<X>")
            );

            // Oversized JS module (> 2 MiB) must be rejected by SandboxedLoader.
            let big_path = canon_plugin.join("huge.js");
            let big_content = format!("export const s = \"{}\";", "a".repeat(MAX_MODULE_BYTES as usize + 64));
            fs::write(&big_path, big_content).unwrap();
            let mut loader = SandboxedLoader::new(canon_plugin.clone());
            assert!(loader.load(&ctx, &big_path.to_string_lossy(), None).is_err());
        });
    }

    #[test]
    fn fern_ui_module_preserves_js_handler_functions_without_eager_registration() {
        let tmp = tempfile::tempdir().unwrap();
        let canon = tmp.path().canonicalize().unwrap();
        let rt = Runtime::new().unwrap();
        rt.set_loader(
            SandboxedResolver::new(canon.clone()),
            SandboxedLoader::new(canon),
        );
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            // If `fern:ui` eagerly called `__fern_reg_handler`, setting it to throw would fail.
            ctx.eval::<(), _>(
                r#"
                globalThis.__fern_reg_handler = () => {
                    throw new Error("must not eagerly register handlers during node creation");
                };
                "#,
            )
            .unwrap();
            let module = Module::declare(
                ctx.clone(),
                "test_ui.js",
                r#"
                import { vstack, button } from "fern:ui";
                let calls = 0;
                const tree = vstack([
                    button({ label: "Click", onClick: () => { calls += 1; return calls; } })
                ]);
                export const handlerType = typeof tree.children[0].onClick;
                export const firstCall = tree.children[0].onClick();
                export const secondCall = tree.children[0].onClick();
                "#,
            )
            .unwrap();
            let (evaluated, promise) = module.eval().unwrap();
            let _ = promise.finish::<()>();
            let handler_type: String = evaluated.get("handlerType").unwrap();
            let first_call: i32 = evaluated.get("firstCall").unwrap();
            let second_call: i32 = evaluated.get("secondCall").unwrap();
            assert_eq!(handler_type, "function");
            assert_eq!(first_call, 1);
            assert_eq!(second_call, 2);
        });
    }
}
