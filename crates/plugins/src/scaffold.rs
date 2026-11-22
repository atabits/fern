//! Plugin template generator (`manifest.json`, working `index.js`, SVG icon, `fern.d.ts`, `jsconfig.json`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};

use crate::manifest::validate_plugin_id;

/// Embedded TypeScript ambient declarations for Fern plugin authors (`fern.d.ts`).
pub const FERN_D_TS: &str = r#"declare module "fern" {
  export type Priority = "none" | "low" | "medium" | "high" | 0 | 1 | 3 | 5;
  export type TaskKind = "task" | "note";
  export type ScheduleBucket = "none" | "evening" | "someday";

  export interface Task {
    id: string;
    title: string;
    notes: string;
    listId: string | null;
    parentId: string | null;
    sectionId: string | null;
    kind: TaskKind;
    priority: "none" | "low" | "medium" | "high";
    dueDate: string | null;
    dueTime: string | null;
    endDate: string | null;
    endTime: string | null;
    startDate: string | null;
    scheduleBucket: ScheduleBucket;
    repeat: string | null;
    tags: string[];
    checklist: { done: number; total: number } | null;
    completed: boolean;
    completedAt: number | null;
    deletedAt: number | null;
    wontDoAt: number | null;
    inProgressAt: number | null;
    pinnedAt: number | null;
    createdAt: number;
    modifiedAt: number | null;
    location: string | null;
    timezone: string | null;
    isFloating: boolean;
    constantReminder: boolean;
    reminders: number[];
  }

  export interface TaskDraft {
    title: string;
    notes?: string;
    listId?: string | null;
    sectionId?: string | null;
    kind?: TaskKind;
    priority?: Priority;
    dueDate?: string | null;
    dueTime?: string | null;
    endDate?: string | null;
    endTime?: string | null;
    startDate?: string | null;
    scheduleBucket?: ScheduleBucket | "";
    repeat?: string | null;
    tags?: string[];
    checklist?: Array<string | { title: string; completed?: boolean }>;
    location?: string | null;
    timezone?: string | null;
    isFloating?: boolean;
    constantReminder?: boolean;
    reminders?: number[];
    top?: boolean;
  }

  export interface TaskPatch extends Omit<Partial<TaskDraft>, "top" | "scheduleBucket"> {
    scheduleBucket?: ScheduleBucket | "" | null;
    checklist?: Array<string | { title: string; completed?: boolean }>;
    completed?: boolean;
    pinned?: boolean;
    inProgress?: boolean;
    wontDo?: boolean;
    parentId?: string | null;
  }

  export interface TaskFilter {
    view?: "inbox" | "all" | "today" | "tomorrow" | "next_7_days" | "completed" | "wont_do" | "trash";
    listId?: string;
    tag?: string;
    completed?: boolean;
    includeDeleted?: boolean;
    search?: string;
  }

  export interface TaskList {
    id: string;
    name: string;
    color: string | null;
    openCount: number;
    icon: string | null;
    folderId: string | null;
    kind: TaskKind;
    inSmartLists: boolean;
    archived: boolean;
    notes: string;
    deadline: string | null;
    completedAt: number | null;
  }

  export interface ListDraft {
    name: string;
    color?: string | null;
    icon?: string | null;
    folderId?: string | null;
    kind?: TaskKind;
    inSmartLists?: boolean;
    notes?: string;
    deadline?: string | null;
  }

  export interface SidebarContribution {
    id: string;
    title: string;
    icon?: string | null;
  }

  /** Dedicated plugin screen opened from Settings -> Plugins without cluttering the sidebar. */
  export interface PageContribution {
    id: string;
    title: string;
    icon?: string | null;
  }

  export interface CommandContribution {
    id: string;
    title: string;
  }

  export interface SearchProviderContribution {
    id: string;
    title: string;
    prefix?: string | null;
  }

  export interface PluginContributions {
    sidebar?: SidebarContribution[];
    pages?: PageContribution[];
    commands?: CommandContribution[];
    search_providers?: SearchProviderContribution[];
    themes?: Array<{ id: string; name: string; appearance?: string; path?: string; colors?: Record<string, unknown> }>;
    locales?: Array<{ code: string; name: string; path?: string; strings?: Record<string, unknown> }>;
    importers?: Array<{ id: string; name: string; extensions?: string[] }>;
  }

  export interface SearchHit {
    id?: string;
    title: string;
    subtitle?: string;
    badge?: string;
    taskId?: string;
    commandId?: string;
    commandArg?: string;
  }

  export interface HttpResponse {
    ok: boolean;
    status: number;
    url: string;
    headers: Record<string, string>;
    body: string;
    text(): string;
    json<T = unknown>(): T;
  }

  export const tasks: {
    list(filter?: TaskFilter): Task[];
    get(id: string): Task | null;
    create(draft: string | TaskDraft): Task;
    update(id: string, patch: TaskPatch): Task;
    complete(id: string): Task & { copyId?: string; cascadedIds?: string[] };
    reopen(id: string): Task;
    delete(id: string): boolean;
    subtasks(id: string): Task[];
    addSubtask(parentId: string, title: string): Task;
    checklist(id: string): Array<{ id: string; taskId: string; title: string; completed: boolean }>;
  };

  export const lists: {
    all(): TaskList[];
    folders(): Array<{ id: string; name: string }>;
    sections(listId?: string | null): Array<{ id: string; listId: string | null; name: string }>;
    create(draft: string | ListDraft): TaskList;
    rename(id: string, name: string): boolean;
  };

  export const tags: {
    all(): Array<{ name: string; color: string | null; openCount: number; parent: string | null; pinned: boolean }>;
    setTaskTags(taskId: string, tags: string[]): boolean;
  };

  export const habits: {
    all(): Array<{ id: string; name: string; icon: string | null; color: string | null; startDate: string | null; goalDays: number | null; archived: boolean; goal: { amount: number; unit: string } | null; section: string }>;
    stats(id: string): { totalDays: number; streak: number; bestStreak: number };
    checkin(id: string, date: string, status: "done" | "undone" | "skip" | "reset" | "toggle" | boolean, amount?: number | null): boolean;
  };

  export const focus: {
    sessions(range?: { from?: string | number; to?: string | number } | null): Array<{ id: string; taskId: string | null; habitId: string | null; title: string; startedAt: number; endedAt: number; minutes: number; stopwatch: boolean; note: string }>;
    stats(range?: { from?: string | number; to?: string | number } | null): { todayCount: number; todayMinutes: number; totalCount: number; totalMinutes: number };
  };

  export const countdowns: {
    all(): Array<{ id: string; name: string; date: string; kind: string; icon: string | null; color: string | null; archived: boolean; pinnedAt: number | null; totalDays: number; past: boolean }>;
    create(draft: { name: string; date: string; kind?: "countdown" | "anniversary" | "birthday" | "holiday" }): { id: string; name: string; date: string; kind: string };
    delete(id: string): boolean;
  };

  export function batch<T>(fn: () => T): T;

  export const storage: {
    get<T = unknown>(key: string): T | null;
    set(key: string, value: unknown): boolean;
    delete(key: string): boolean;
    keys(prefix?: string | null): string[];
  };

  export const meta: {
    get<T = unknown>(entityType: "task" | "list" | "habit" | "countdown" | "day", entityId: string, key: string): T | null;
    set(entityType: "task" | "list" | "habit" | "countdown" | "day", entityId: string, key: string, value: unknown): boolean;
    list(entityType: "task" | "list" | "habit" | "countdown" | "day", entityId: string): Record<string, unknown>;
    delete(entityType: "task" | "list" | "habit" | "countdown" | "day", entityId: string, key: string): boolean;
  };

  export const settings: {
    get<T = unknown>(key: string): T | null;
    all(): Record<string, unknown>;
    hasSecret(key: string): boolean;
    onChange(cb: (nextSettings: Record<string, unknown>) => void): void;
  };

  export interface OAuthTokenSummary {
    ok: boolean;
    tokenType: string | null;
    scope: string | null;
    expiresIn: number | null;
    hasRefreshToken: boolean;
  }

  export const http: {
    fetch(url: string, options?: { method?: string; headers?: Record<string, string>; body?: unknown; timeoutMs?: number }): Promise<HttpResponse>;
  };

  export const oauth: {
    /**
     * Runs a zero-secret OAuth 2.0 loopback flow and saves tokens directly in `SecretsVault`.
     * Default redirect URI: `http://127.0.0.1:18481/callback` (`redirectHost: "127.0.0.1"`,
     * `redirectPort: 18481`, `redirectPath: "/callback"`). `timeoutSec` defaults to `180`.
     */
    authorize(options: {
      authorizeUrl: string;
      tokenUrl: string;
      clientId: string;
      clientSecretKey?: string;
      scopes?: string[] | string;
      /** Loopback callback host (default `"127.0.0.1"`). */
      redirectHost?: "127.0.0.1" | "localhost";
      /** Loopback callback port in `1024..=65535` (default `18481`). */
      redirectPort?: number;
      /** Loopback callback path starting with `/` (default `"/callback"`). */
      redirectPath?: string;
      /** Token endpoint client authentication method (default `"basic"`). */
      tokenAuth?: "basic" | "body";
      /** Enable PKCE S256 `code_challenge` / `code_verifier` (default `false`). */
      pkce?: boolean;
      /** Vault key for access token (default `"access_token"`). */
      saveAccessTokenAs?: string;
      /** Vault key for refresh token (default `"refresh_token"`, `null` to omit). */
      saveRefreshTokenAs?: string | null;
      extraAuthorizeParams?: Record<string, string>;
      /** Browser callback timeout in seconds, clamped to `5..=300` (default `180`). */
      timeoutSec?: number;
    }): Promise<OAuthTokenSummary>;
    refresh(options: {
      tokenUrl: string;
      clientId: string;
      clientSecretKey?: string;
      refreshTokenKey?: string;
      scopes?: string[] | string;
      tokenAuth?: "basic" | "body";
      saveAccessTokenAs?: string;
      saveRefreshTokenAs?: string | null;
    }): Promise<OAuthTokenSummary>;
    /** Removes stored token secrets (defaults to `["access_token", "refresh_token"]`). */
    clearTokens(keys?: string | string[] | null): number;
  };

  export const events: {
    on(kind: string, cb: (event: { kind: string; entityId: string; originPlugin: string | null; depth: number; at: number; entity: unknown }) => void | Promise<void>): () => void;
  };

  export const hooks: {
    beforeTaskCreate(cb: (draft: TaskDraft) => unknown): void;
    beforeTaskComplete(cb: (task: Task) => unknown): void;
    quickAddTransform(cb: (text: string) => unknown): void;
    onReminder(cb: (reminder: unknown) => void): void;
  };

  export const timers: {
    setInterval(id: string, ms: number, cb: () => void | Promise<void>): void;
    clearInterval(id: string): boolean;
  };

  export const commands: {
    register(id: string, handler: (arg?: string) => unknown | Promise<unknown>): void;
  };

  export const search: {
    registerProvider(id: string, handler: (query: string) => SearchHit[] | Promise<SearchHit[]>): void;
  };

  export const services: {
    provide(name: string, fn: (arg: unknown) => unknown | Promise<unknown>): void;
    call<T = unknown>(name: string, arg?: unknown): Promise<T>;
  };

  export const views: {
    /** Registers a view declared in `manifest.contributions.sidebar` or `manifest.contributions.pages`. */
    register(viewId: string, view: { render: () => import("fern:ui").UiNode; mount?: () => void; unmount?: () => void }): void;
    /** Requests a live re-render of a registered sidebar or page view (e.g. from timers or async tasks). */
    refresh(viewId: string): void;
  };

  export const ui: {
    toast(message: string, level?: "info" | "success" | "warning" | "error" | null): void;
    confirm(title: string, body: string): Promise<boolean>;
    prompt(title: string, placeholder?: string | null, defaultValue?: string | null): Promise<string | null>;
    /**
     * Opens a modal displaying a one-shot `UiNode` tree.
     * The host does not invoke `mount()`/`unmount()` or auto-re-render on clicks; call `openModal`
     * again to update contents or use `openModalView()` from `@fern/sdk`.
     */
    openModal(title: string, viewOrNode: import("fern:ui").UiNode | (() => import("fern:ui").UiNode) | { render: () => import("fern:ui").UiNode }): void;
    closeModal(): void;
    openTask(taskId: string): void;
    /** Opens Fern's Settings window directly to this plugin's settings card. */
    openSettings(): void;
  };

  export const system: {
    locale(): string;
    themeAppearance(): "light" | "dark";
    openExternal(url: string): void;
    clipboardWrite(text: string): void;
  };

  const fern: {
    tasks: typeof tasks;
    lists: typeof lists;
    tags: typeof tags;
    habits: typeof habits;
    focus: typeof focus;
    countdowns: typeof countdowns;
    batch: typeof batch;
    storage: typeof storage;
    meta: typeof meta;
    settings: typeof settings;
    http: typeof http;
    oauth: typeof oauth;
    events: typeof events;
    hooks: typeof hooks;
    timers: typeof timers;
    commands: typeof commands;
    search: typeof search;
    services: typeof services;
    views: typeof views;
    ui: typeof ui;
    system: typeof system;
  };
  export default fern;
}

declare module "fern:ui" {
  export type UiNode = Record<string, unknown>;
  export function vstack(propsOrChildren: Record<string, unknown> | UiNode[], children?: UiNode[]): UiNode;
  export function hstack(propsOrChildren: Record<string, unknown> | UiNode[], children?: UiNode[]): UiNode;
  export function grid(propsOrChildren: Record<string, unknown> | UiNode[], children?: UiNode[]): UiNode;
  export function scroll(props: { key: string; maxHeight?: number; child?: UiNode }, children?: UiNode[]): UiNode;
  export function card(propsOrChildren: Record<string, unknown> | UiNode[], children?: UiNode[]): UiNode;
  export function section(titleOrProps: string | Record<string, unknown>, children?: UiNode[]): UiNode;
  export function divider(): UiNode;
  export function spacer(size?: number | { size?: number }): UiNode;
  export function heading(textOrProps: string | { text: string; level?: number }, level?: number): UiNode;
  export function text(textOrProps: string | { text: string; muted?: boolean; weight?: string; color?: string; size?: string }): UiNode;
  export function markdown(contentOrProps: string | { content: string }): UiNode;
  export function keyValue(props: { label: string; value: string; muted?: boolean }): UiNode;
  export function badge(labelOrProps: string | { label: string; color?: string }): UiNode;
  export function progress(valueOrProps: number | { value: number; label?: string; color?: string }): UiNode;
  export function donut(valueOrProps: number | { value: number; label?: string; size?: number; color?: string }): UiNode;
  export function emptyState(props: { icon?: string; title: string; subtitle?: string; actionLabel?: string; onAction?: () => void | Promise<void> }): UiNode;
  export function svg(props: { content: string; width?: number; height?: number }): UiNode;
  export function button(props: { label: string; variant?: "primary" | "secondary" | "destructive" | "ghost"; size?: "sm" | "md"; icon?: string; disabled?: boolean; loading?: boolean; onClick?: () => void | Promise<void> }): UiNode;
  export function iconButton(props: { icon: string; tooltip?: string; disabled?: boolean; onClick?: () => void | Promise<void> }): UiNode;
  export function input(props: { key: string; value: string; revision?: number; placeholder?: string; disabled?: boolean; onChange?: (value: string) => void | Promise<void>; onSubmit?: (value: string) => void | Promise<void> }): UiNode;
  export function textArea(props: { key: string; value: string; revision?: number; rows?: number; placeholder?: string; disabled?: boolean; onChange?: (value: string) => void | Promise<void> }): UiNode;
  export function select(props: { key: string; value: string; options: Array<{ label: string; value: string }>; disabled?: boolean; onChange?: (value: string) => void | Promise<void> }): UiNode;
  export function checkbox(props: { key?: string; checked: boolean; label?: string; disabled?: boolean; onChange?: (checked: boolean) => void | Promise<void> }): UiNode;
  export function toggle(props: { key?: string; checked: boolean; label?: string; disabled?: boolean; onChange?: (checked: boolean) => void | Promise<void> }): UiNode;
  export const switchControl: typeof toggle;
  export function segmentedTabs(props: { key?: string; value: string; options: Array<{ label: string; value: string }>; onChange?: (value: string) => void | Promise<void> }): UiNode;
  export function listItem(props: { key: string; title: string; subtitle?: string; badge?: string; trailing?: string; selected?: boolean; onClick?: () => void | Promise<void> }): UiNode;
  export function registerHandler(fn: (...args: any[]) => unknown): number;

  const ui: {
    vstack: typeof vstack;
    hstack: typeof hstack;
    grid: typeof grid;
    scroll: typeof scroll;
    card: typeof card;
    section: typeof section;
    divider: typeof divider;
    spacer: typeof spacer;
    heading: typeof heading;
    text: typeof text;
    markdown: typeof markdown;
    keyValue: typeof keyValue;
    badge: typeof badge;
    progress: typeof progress;
    donut: typeof donut;
    emptyState: typeof emptyState;
    svg: typeof svg;
    button: typeof button;
    iconButton: typeof iconButton;
    input: typeof input;
    textArea: typeof textArea;
    select: typeof select;
    checkbox: typeof checkbox;
    switch: typeof toggle;
    toggle: typeof toggle;
    segmentedTabs: typeof segmentedTabs;
    listItem: typeof listItem;
    registerHandler: typeof registerHandler;
  };
  export default ui;
}

interface Console {
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  debug(...args: unknown[]): void;
}

declare var console: Console;
"#;

/// Embedded `jsconfig.json` enabling IDE TypeScript/JSDoc typechecking against `fern.d.ts`.
pub const JSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022"],
    "module": "ES2022",
    "moduleResolution": "bundler",
    "checkJs": true,
    "allowJs": true,
    "noEmit": true
  },
  "include": ["*.js", "fern.d.ts"]
}
"#;

const SAMPLE_PLUGIN_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M12 2L15 8.5L22 9.3L17 14L18.2 21L12 17.5L5.8 21L7 14L2 9.3L9 8.5Z"/>
</svg>
"#;

/// Generates a working sample plugin directory with a sidebar view, command, event hook,
/// SVG icon, `fern.d.ts`, and `jsconfig.json`.
pub fn generate_sample_plugin(
    plugins_dir: &Path,
    plugin_id: &str,
    name: &str,
) -> Result<PathBuf> {
    ensure!(
        validate_plugin_id(plugin_id),
        "invalid plugin id {plugin_id:?}"
    );
    let display_name = if name.trim().is_empty() {
        "Sample Plugin"
    } else {
        name.trim()
    };

    let target = plugins_dir.join(plugin_id);
    ensure!(
        !target.exists(),
        "plugin directory already exists: {}",
        target.display()
    );
    fs::create_dir_all(target.join("icons"))
        .with_context(|| format!("cannot create plugin directory {}", target.display()))?;

    let manifest_json = serde_json::json!({
        "id": plugin_id,
        "name": display_name,
        "version": "0.1.0",
        "api_version": 1,
        "min_fern_version": "0.2.0",
        "description": "Starter Fern plugin with a sidebar view, command, and task event hook.",
        "author": "Fern User",
        "entry": "index.js",
        "permissions": ["tasks:read", "tasks:write", "storage"],
        "activation_events": ["onView:overview", "onCommand:quick_note", "onEvent:task.created"],
        "contributions": {
            "sidebar": [
                { "id": "overview", "title": display_name, "icon": "icons/plugin.svg" }
            ],
            "commands": [
                { "id": "quick_note", "title": format!("{display_name}: Create Sample Task") }
            ]
        },
        "settings": [
            { "key": "prefix", "type": "text", "label": "Task Prefix", "default": "[Plugin]" }
        ]
    });

    let heading_literal = serde_json::to_string(display_name)?;
    let index_js = format!(
        r#"import fern from "fern";
import * as ui from "fern:ui";

let draftTitle = "";
let draftRev = 1;

fern.views.register("overview", {{
  render() {{
    const openTasks = fern.tasks.list({{ completed: false }});
    const createdCount = fern.storage.get("created_count") ?? 0;

    return ui.vstack({{ gap: 12, padding: 16 }}, [
      ui.heading({heading_literal}, 2),
      ui.hstack({{ gap: 12 }}, [
        ui.card({{ title: "Open Tasks" }}, [
          ui.text({{ text: String(openTasks.length), weight: "bold", size: "lg" }}),
        ]),
        ui.card({{ title: "Plugin Created" }}, [
          ui.text({{ text: String(createdCount), weight: "bold", size: "lg" }}),
        ]),
      ]),
      ui.hstack({{ gap: 8 }}, [
        ui.input({{
          key: "sample-task-input",
          value: draftTitle,
          revision: draftRev,
          placeholder: "New task title...",
          onChange(next) {{
            draftTitle = next;
          }},
          onSubmit(val) {{
            const text = (val ?? draftTitle).trim();
            if (!text) return;
            const prefix = fern.settings.get("prefix") ?? "[Plugin]";
            fern.tasks.create({{ title: `${{prefix}} ${{text}}` }});
            draftTitle = "";
            draftRev += 1;
            fern.ui.toast("Task created from plugin", "success");
          }},
        }}),
        ui.button({{
          label: "Add Task",
          variant: "primary",
          onClick() {{
            const text = draftTitle.trim();
            if (!text) return;
            const prefix = fern.settings.get("prefix") ?? "[Plugin]";
            fern.tasks.create({{ title: `${{prefix}} ${{text}}` }});
            draftTitle = "";
            draftRev += 1;
            fern.ui.toast("Task created from plugin", "success");
          }},
        }}),
      ]),
    ]);
  }},
}});

fern.commands.register("quick_note", () => {{
  const prefix = fern.settings.get("prefix") ?? "[Plugin]";
  const task = fern.tasks.create({{ title: `${{prefix}} Quick item` }});
  fern.ui.toast(`Created: ${{task.title}}`, "info");
  return task.id;
}});

fern.events.on("task.created", (ev) => {{
  const count = (fern.storage.get("created_count") ?? 0) + 1;
  fern.storage.set("created_count", count);
  console.log("Observed task.created:", ev.entityId);
}});
"#
    );

    fs::write(
        target.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_json)?,
    )?;
    fs::write(target.join("icons/plugin.svg"), SAMPLE_PLUGIN_SVG)?;
    fs::write(target.join("index.js"), index_js)?;
    fs::write(target.join("fern.d.ts"), FERN_D_TS)?;
    fs::write(target.join("jsconfig.json"), JSCONFIG_JSON)?;

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_plugin_escapes_display_name_and_includes_updated_declarations() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let dir = generate_sample_plugin(
            tmp.path(),
            "com.example.escaped",
            "My \"Quoted\" \\ Plugin\nLine2",
        )?;
        let js = fs::read_to_string(dir.join("index.js"))?;
        assert!(
            js.contains(r#"ui.heading("My \"Quoted\" \\ Plugin\nLine2", 2),"#),
            "expected escaped heading literal, got:\n{js}"
        );
        let dts = fs::read_to_string(dir.join("fern.d.ts"))?;
        assert!(dts.contains("export default ui;"));
        assert!(dts.contains("export const switchControl: typeof toggle;"));
        assert!(dts.contains("export function registerHandler"));
        assert!(dts.contains("declare var console: Console;"));
        assert!(dts.contains("18481"));

        let jsconfig: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("jsconfig.json"))?)?;
        assert_eq!(jsconfig["compilerOptions"]["lib"], serde_json::json!(["ES2022"]));
        Ok(())
    }
}
