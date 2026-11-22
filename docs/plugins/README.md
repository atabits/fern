# Fern Plugin & Script Developer Guide

Fern includes a sandboxed **QuickJS-NG** (ES2022) plugin engine and built-in **Script Runner** so you can extend tasks, lists, habits, focus sessions, and the sidebar with native GPUI views — with no browser DOM, no Node.js runtime dependency, and strict capability permissions.

Use this guide whether you are writing a lightweight zero-build JavaScript plugin, building a TypeScript + JSX panel with [`@fern/sdk`](../../sdk/fern-sdk/package.json), automating workflows with **Script Runner**, or using an AI assistant to author plugins.

---

## Table of Contents

1. [How Plugins Work in Fern](#how-plugins-work-in-fern)
2. [`manifest.json` Contract (`api_version: 1`)](#manifestjson-contract-api_version-1)
3. [Pure JS Quick-Start (Zero Build Step)](#pure-js-quick-start-zero-build-step)
4. [TypeScript + `@fern/sdk` JSX Workflow](#typescript--fernsdk-jsx-workflow)
5. [Declarative UI Primitives (`UiNode`)](#declarative-ui-primitives-uinode)
6. [Host API Reference (`fern`)](#host-api-reference-fern)
7. [Script Runner Quick Recipes](#script-runner-quick-recipes)
8. [Security Model & Runtime Quotas](#security-model--runtime-quotas)
9. [AI-Authoring Checklist](#ai-authoring-checklist)
10. [Publishing to `atabits/fern-plugins` & Custom Registries](#publishing-to-atabitsfern-plugins--custom-registries)

---

## How Plugins Work in Fern

- **Directory layout:** Installed plugins live at `<FERN_DATA_DIR>/plugins/<plugin_id>/` (the directory name must match `manifest.json`'s `"id"`). You can inspect or create a starter plugin directly from **Settings → Plugins → Installed → New Plugin Template** or **Open Plugins Folder**.
- **Runtime:** Every enabled plugin runs in its own isolated `rquickjs::AsyncRuntime` on Fern's single `PluginWorker` OS thread.
- **No DOM / No Node.js:** Only ES2022 standard built-ins, `console`, and the virtual ES modules `"fern"` and `"fern:ui"` (plus relative `./` / `../` JS modules inside your plugin folder) are available.
- **Native GPUI rendering:** Plugins never render HTML. Your view returns a declarative `UiNode` tree that Fern's `plugin_ui` host renders natively using Fern's theme tokens and GPUI components.
- **Safe mode:** Launching `fern --safe-mode` (or enabling Safe Mode in Settings) skips loading all plugins at startup.
- **Hot reload:** When hot reload is active, editing `manifest.json`, `.js`, `.mjs`, `.svg`, or `.json` files inside a plugin directory reloads the plugin automatically.
- **Deep links:** External links of the form `fern://plugin/<plugin_id>/<command>?key=value` invoke the matching registered command.

---

## `manifest.json` Contract (`api_version: 1`)

Every plugin requires a `manifest.json` at its root directory:

```json
{
  "id": "com.example.github-issues",
  "name": "GitHub Issues",
  "version": "1.0.0",
  "api_version": 1,
  "min_fern_version": "0.2.0",
  "description": "Sync assigned GitHub issues and view them in a sidebar panel.",
  "author": "Your Name",
  "homepage": "https://github.com/example/fern-github-issues",
  "entry": "index.js",
  "permissions": [
    "tasks:read",
    "tasks:write",
    "lists:read",
    "storage",
    "meta",
    { "network": ["api.github.com"] }
  ],
  "activation_events": [
    "onView:issues",
    "onCommand:sync",
    "onEvent:task.completed"
  ],
  "contributions": {
    "sidebar": [
      { "id": "issues", "title": "GitHub", "icon": "icons/github.svg" }
    ],
    "commands": [
      { "id": "sync", "title": "GitHub: Sync Issues Now" }
    ],
    "search_providers": [
      { "id": "gh-search", "title": "GitHub Issues", "prefix": "gh:" }
    ]
  },
  "settings": [
    {
      "key": "repo",
      "type": "text",
      "label": "Repository",
      "placeholder": "owner/repo",
      "default": ""
    },
    {
      "key": "interval_min",
      "type": "number",
      "label": "Sync interval (minutes)",
      "default": 15,
      "min": 1,
      "max": 1440,
      "step": 5
    },
    {
      "key": "auto_close",
      "type": "boolean",
      "label": "Close issue when task completes",
      "default": false
    },
    {
      "key": "mode",
      "type": "select",
      "label": "Filter Mode",
      "options": [
        { "label": "Assigned to me", "value": "assigned" },
        { "label": "All open issues", "value": "all" }
      ],
      "default": "assigned"
    },
    {
      "key": "token",
      "type": "secret",
      "label": "Personal Access Token"
    }
  ]
}
```

### Required Fields

| Field | Rule |
| --- | --- |
| `id` | String matching `^[a-z0-9][a-z0-9._-]{1,63}$` (2–64 chars), no `..`, must match folder name. |
| `name` | Non-empty display name. |
| `version` | Non-empty version string (e.g. `"1.0.0"`). |
| `api_version` | Must be `1`. |
| `entry` | Clean relative path to an ES module inside the plugin directory (e.g. `"index.js"` or `"dist/index.js"`). |

### Permissions

Capabilities are **deny-by-default** and require explicit user approval before first activation (and re-approval if an update adds new capabilities):

| Permission | Grants |
| --- | --- |
| `"tasks:read"` | `fern.tasks.list`, `get`, `subtasks`, `checklist`, and `task.*` events |
| `"tasks:write"` | `fern.tasks.create`, `update`, `complete`, `reopen`, `delete`, `addSubtask` |
| `"lists:read"` / `"lists:write"` | `fern.lists.all`, `folders`, `sections` / `create`, `rename` |
| `"tags:read"` / `"tags:write"` | `fern.tags.all` / `setTaskTags` |
| `"habits:read"` / `"habits:write"` | `fern.habits.all`, `stats`, `habit.checkin` event / `checkin` |
| `"focus:read"` | `fern.focus.sessions`, `stats`, `focus.completed` event |
| `"countdowns:read"` / `"countdowns:write"` | `fern.countdowns.all` / `create`, `delete` |
| `"storage"` | Per-plugin KV store (`fern.storage`, 5 MiB total cap) |
| `"meta"` | Per-entity metadata (`fern.meta`, 64 KiB cap per entry) |
| `"notifications"` | Reserved for notification capabilities |
| `"clipboard"` | `fern.system.clipboardWrite` |
| `{ "network": ["api.github.com", "*.openai.com"] }` | Outbound HTTP(S) allowlist for `fern.http.fetch` and `fern.oauth` endpoints |
| `"network:local"` | Allows loopback hosts (`127.0.0.1`, `::1`, `localhost`) directly, and allows private LAN ranges (RFC1918/CGNAT/ULA) **when also matched by a `"network"` host rule** |
| `"hooks:intercept"` | `fern.hooks.beforeTaskCreate`, `beforeTaskComplete`, `quickAddTransform`, `onReminder` |
| `"services"` | Inter-plugin RPC via `fern.services.provide` and `fern.services.call` |

#### `network` Allowlist & `network:local` Rules

- **Exact match:** `"api.github.com"` allows only `api.github.com`.
- **Subdomain wildcard:** `"*.github.com"` matches `api.github.com` and `sub.api.github.com`, **not** bare `github.com`.
- **Port restriction:** `"api.openai.com:443"` or `"192.168.1.50:8080"` locks requests to that port.
- **Loopback access (`127.0.0.1`, `::1`, `localhost`):** Granting `"network:local"` alone permits loopback host literals (`127.0.0.1`, `::1`, `localhost` — not `*.localhost`). Note that **`fern.oauth` does not require `"network:local"`** for its built-in loopback callback server (`http://127.0.0.1:18481/callback`).
- **Private / LAN access:** Connecting to RFC1918/CGNAT/ULA LAN addresses (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10`, `fc00::/7`) or LAN hostnames resolving to them requires **both** a matching host pattern in `{"network": [...]}` **and** `"network:local"`:

```json
"permissions": [
  { "network": ["192.168.1.50:11434"] },
  "network:local"
]
```

Cloud link-local metadata ranges (`169.254.0.0/16`, `fe80::/10`), unspecified (`0.0.0.0/8`, `::`), IPv6 transition prefixes (`64:ff9b::/96`, `2002::/16`, `2001::/32`), and multicast/reserved ranges are **always blocked** even when `"network:local"` is granted.

### Activation Events

If `activation_events` is omitted or empty, the plugin activates whenever eligible. Otherwise, declare only the triggers you need so Fern loads plugins lazily:

- `"*"` or `"onStartup"` — activate immediately after worker startup
- `"onView:<viewId>"` — activate when the user opens sidebar or page view `<viewId>`
- `"onCommand:<commandId>"` — activate when command `<commandId>` runs
- `"onEvent:<eventKind>"` or `"onEvent:*"` — activate on matching domain events (e.g. `"onEvent:task.created"`, `"onEvent:app.ready"`)
- `"onHook:beforeTaskCreate"`, `"onHook:beforeTaskComplete"`, `"onHook:quickAddTransform"`, `"onHook:onReminder"`

### Contributions & Settings

- **`contributions.sidebar`**: `[{ "id": "<viewId>", "title": "...", "icon": "icons/icon.svg" }]` — adds a dedicated view to the main sidebar rail.
- **`contributions.pages`**: `[{ "id": "<viewId>", "title": "...", "icon": "icons/icon.svg" }]` — adds a dedicated plugin screen opened from **Settings → Plugins** without adding a sidebar rail icon (ideal for sync dashboards, setup wizards, or admin tools).
  - View IDs must be unique across **both** `sidebar` and `pages`, must not contain `/`, and must not use reserved internal scopes (`__modal__`, `__default__`). `icon` is an optional relative path to an SVG (`<= 256 KiB`, sanitized before rendering).
- **`contributions.commands`**: `[{ "id": "<commandId>", "title": "..." }]` — surfaced in Command Search and deep links.
- **`contributions.search_providers`**: `[{ "id": "<providerId>", "title": "...", "prefix": "gh:" }]`.
- **`contributions.themes` / `locales` / `importers`**: declarative theme JSON/colors, locale string tables, and importer declarations.
- **Reserved Yellow-Zone fields:** `task_badges`, `task_actions`, `virtual_lists`, and `task_detail_sections` are accepted by the manifest validator for forward compatibility.
- **`settings`**: auto-generates the plugin's form in **Settings → Plugins → Installed**:
  - `type`: `"text"` | `"number"` | `"boolean"` | `"select"` | `"secret"`
  - `"select"` requires non-empty `"options"` (either `"a"` strings or `{ "label": "A", "value": "a" }` objects).
  - `"secret"` values are stored in the encrypted `SecretsVault` (`plugin-secrets.vault`), **never** in `tasks.db` or `settings_json`. Calling `fern.settings.get(secretKey)` throws intentionally — use `fern.settings.hasSecret(key)` to check configuration and `${secret:key}` inside `fern.http.fetch` headers to use the secret without exposing it to JS.

---

## Pure JS Quick-Start (Zero Build Step)

Create `<FERN_DATA_DIR>/plugins/com.example.hello/`:

```text
com.example.hello/
├── manifest.json
├── index.js
├── fern.d.ts        (optional: IDE autocomplete via Fern's template)
└── jsconfig.json    (optional: enables checkJs in editors)
```

**`manifest.json`**

```json
{
  "id": "com.example.hello",
  "name": "Hello Fern",
  "version": "0.1.0",
  "api_version": 1,
  "entry": "index.js",
  "permissions": ["tasks:read", "tasks:write"],
  "activation_events": ["onView:overview", "onCommand:quick_hello"],
  "contributions": {
    "sidebar": [{ "id": "overview", "title": "Hello" }],
    "commands": [{ "id": "quick_hello", "title": "Hello: Create Sample Task" }]
  }
}
```

**`index.js`**

```js
import fern from "fern";
import * as ui from "fern:ui";

let draftTitle = "";
let draftRev = 1;

export function activate() {
  fern.views.register("overview", {
    render() {
      const openTasks = fern.tasks.list({ completed: false });

      return ui.vstack({ gap: 12, padding: 16 }, [
        ui.heading("Hello Fern", 2),
        ui.text({ text: `Open tasks: ${openTasks.length}`, muted: true }),
        ui.hstack({ gap: 8 }, [
          ui.input({
            key: "hello-input",
            value: draftTitle,
            revision: draftRev,
            placeholder: "New task title...",
            onChange(next) {
              draftTitle = next;
            },
            onSubmit(val) {
              const title = (val ?? draftTitle).trim();
              if (!title) return;
              fern.tasks.create({ title });
              draftTitle = "";
              draftRev += 1;
              fern.ui.toast("Task created", "success");
            },
          }),
          ui.button({
            label: "Add",
            variant: "primary",
            onClick() {
              const title = draftTitle.trim();
              if (!title) return;
              fern.tasks.create({ title });
              draftTitle = "";
              draftRev += 1;
              fern.ui.toast("Task created", "success");
            },
          }),
        ]),
      ]);
    },
  });

  fern.commands.register("quick_hello", () => {
    const task = fern.tasks.create({ title: "Hello from command" });
    fern.ui.toast(`Created: ${task.title}`, "info");
    return task.id;
  });
}

export function deactivate() {
  console.log("Hello Fern deactivated");
}
```

> [!TIP]
> **`Input` & `TextArea` `revision` rule:** Fern keeps `TextInput` responsive at native FPS while debouncing `onChange` back to JS. Native text state is overwritten from JS **only** when `revision` increments (for example after clearing an input on submit), so typing never steals focus or jumps the cursor.

---

## TypeScript + `@fern/sdk` JSX Workflow

For larger plugins, [`sdk/fern-sdk/`](../../sdk/fern-sdk/package.json) provides:

- Full TypeScript definitions (`fern.d.ts` synced with Fern's host bindings)
- Automatic JSX runtime (`jsx`, `jsxs`, `Fragment`) compiling JSX into `"fern:ui"` `UiNode` calls
- Typed PascalCase wrappers (`VStack`, `HStack`, `Button`, `Input`, `Switch`, …) **and** lowercase intrinsic JSX tags (`<vstack>`, `<button>`, …)
- Lightweight QuickJS-native `defineView`, `useState`, `useEffect`, `useRef`, `useMemo`, and `useCallback`
- Zero DOM/browser assumptions (`lib: ["ES2022"]`)

See the complete working example in [`sdk/fern-sdk/example/`](../../sdk/fern-sdk/example/src/index.tsx).

### 1. Project Setup

`tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "jsx": "react-jsx",
    "jsxImportSource": "@fern/sdk",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*.ts", "src/**/*.tsx"]
}
```

`build.mjs` (using `esbuild`):

```js
import * as esbuild from "esbuild";

await esbuild.build({
  entryPoints: ["src/index.tsx"],
  outfile: "dist/index.js",
  bundle: true,
  format: "esm",
  target: "es2022",
  platform: "neutral",
  mainFields: ["module", "main"],
  external: ["fern", "fern:ui"],
  jsx: "automatic",
  jsxImportSource: "@fern/sdk",
});
```

Set `"entry": "dist/index.js"` in `manifest.json`. Keep `"fern"` and `"fern:ui"` external so QuickJS resolves them directly to Fern's built-in host modules, and keep the bundled `dist/index.js` under QuickJS's **2 MiB** per-module limit.

### 2. Reactive Views with `defineView`, `useState`, and `useEffect`

```tsx
import fern from "fern";
import {
  Button,
  Heading,
  Input,
  ListItem,
  Scroll,
  VStack,
  defineView,
  useEffect,
  useState,
} from "@fern/sdk";

function InboxTriageView() {
  const [query, setQuery] = useState("");
  const [queryRev, setQueryRev] = useState(1);
  const [lastSyncedAt, setLastSyncedAt] = useState<number | null>(null);

  useEffect(() => {
    setLastSyncedAt(fern.storage.get<number>("last_synced_at"));
    return () => {
      // Runs before re-running this effect and when the view unmounts
    };
  }, []);

  const tasks = fern.tasks.list({ view: "inbox", completed: false, search: query });

  return (
    <VStack gap={12} padding={16}>
      <Heading level={2}>Inbox ({tasks.length})</Heading>
      <Input
        key="search-box"
        value={query}
        revision={queryRev}
        placeholder="Filter inbox..."
        onChange={setQuery}
      />
      <Button
        label="Clear"
        variant="ghost"
        onClick={() => {
          setQuery("");
          setQueryRev((r) => r + 1);
        }}
      />
      <Scroll key="inbox-scroll" maxHeight={320}>
        <VStack gap={4}>
          {tasks.map((t) => (
            <ListItem
              key={t.id}
              title={t.title}
              subtitle={lastSyncedAt ? `Last sync: ${lastSyncedAt}` : undefined}
              onClick={() => fern.ui.openTask(t.id)}
            />
          ))}
        </VStack>
      </Scroll>
    </VStack>
  );
}

export function activate(): void {
  fern.views.register("triage", defineView(InboxTriageView));
}
```

#### How Re-Renders Work in QuickJS

- **Mount & refresh:** `mount_view` invokes `mount()` then `render()`. `useEffect(..., [])` runs after the first render pass; synchronous `setState` calls inside `useEffect` trigger an immediate re-render pass (capped at 25 passes) before returning the `UiNode` tree to GPUI.
- **UI callbacks:** Whenever the user triggers `onClick`, `onChange`, `onSubmit`, or `onAction` (including `async` handlers that `await fern.http.fetch(...)` or `fern.ui.confirm(...)`), Fern awaits the callback and automatically re-invokes `render()`, emitting the updated `UiNode` tree.
- **Out-of-band updates (`fern.views.refresh` & `onInvalidate`):** State changes outside a UI event handler — such as inside `fern.events.on(...)`, `fern.settings.onChange(...)`, `fern.timers.setInterval(...)`, or intermediate `setLoading(true)` calls *before* an `await` — do **not** trigger a host re-render automatically. Call `fern.views.refresh("<viewId>")` after updating state (or pass `{ onInvalidate: () => fern.views.refresh("<viewId>") }` to `defineView`, noting that `onInvalidate` also runs on synchronous click setters and therefore schedules an extra render on clicks; explicit `fern.views.refresh("<viewId>")` calls in async/background callbacks avoid duplicate renders).
- **Reactive modals (`openModalView`):** `fern.ui.openModal(...)` renders a one-shot `UiNode` tree and does not drive `mount()`/`unmount()` or automatic re-renders. Use `openModalView(title, ({ close }) => ...)` and `closeModalView()` from `@fern/sdk` when a modal needs `useState` / `useEffect` reactivity with automatic subscription cleanup.
- **Unmount:** Switching away unmounts the view, runs all `useEffect` cleanup callbacks, ignores stale post-unmount `setState` calls, and resets hook state for the next mount.
- **Hook rules:** Call `useState`, `useEffect`, `useRef`, `useMemo`, and `useCallback` at the top level of your `defineView` component (never conditionally or inside `.map()` loops).
- **Fragments & lists:** `<Fragment>` (`<>...</>`), nested arrays (`.map()`), booleans (`cond && <Badge />`), `null`, and `undefined` (including functional components returning `null`/`undefined`/`false`) flatten cleanly inside layout containers with zero extra spacing.

---

## Declarative UI Primitives (`UiNode`)

All 26 primitives map 1-to-1 to native `crates/ui` GPUI elements:

### Layout

| Component (`@fern/sdk`) | Intrinsic Tag | Props |
| --- | --- | --- |
| `VStack` | `vstack` | `gap?`, `padding?`, `align?: "start" \| "center" \| "end" \| "stretch"`, `children?` |
| `HStack` | `hstack` | `gap?`, `padding?`, `align?`, `justify?: "start" \| "center" \| "end" \| "between"`, `wrap?`, `children?` |
| `Grid` | `grid` | `cols?` *(default 2)*, `gap?`, `children?` |
| `Scroll` | `scroll` | `key` *(required for scroll position persistence)*, `maxHeight?`, `children?` |
| `Card` | `card` | `title?`, `subtitle?`, `padding?`, `children?` |
| `Section` | `section` | `title`, `subtitle?`, `children?` |
| `Divider` | `divider` | *(none)* |
| `Spacer` | `spacer` | `size?` |

### Display

| Component (`@fern/sdk`) | Intrinsic Tag | Props |
| --- | --- | --- |
| `Heading` | `heading` | `text` *(or text children)*, `level?` *(default 2)* |
| `Text` | `text` | `text` *(or text children)*, `muted?`, `weight?: "normal" \| "medium" \| "semibold" \| "bold"`, `color?`, `size?: "xs" \| "sm" \| "md" \| "lg"` |
| `Markdown` | `markdown` | `content` *(or text children)* |
| `KeyValue` | `keyValue` | `label`, `value`, `muted?` |
| `Badge` | `badge` | `label` *(or text children)*, `color?` |
| `Progress` | `progress` | `value` *(0..1)*, `label?`, `color?` |
| `Donut` | `donut` | `value` *(0..1)*, `label?`, `size?`, `color?` |
| `EmptyState` | `emptyState` | `icon?`, `title`, `subtitle?`, `actionLabel?`, `onAction?: () => void \| Promise<void>` |
| `Svg` | `svg` | `content` *(sanitized raw `<svg>` string <= 256 KiB)*, `width?`, `height?` |

### Input & Actions

| Component (`@fern/sdk`) | Intrinsic Tag | Props |
| --- | --- | --- |
| `Button` | `button` | `label` *(or text children)*, `variant?: "primary" \| "secondary" \| "destructive" \| "ghost"`, `size?: "sm" \| "md"`, `icon?`, `disabled?`, `loading?`, `onClick?` |
| `IconButton` | `iconButton` | `icon` *(snake_case Fern icon name, e.g. `"plus"`, `"trash"`, `"refresh_cw"`, `"check"`, `"search"`)*, `tooltip?`, `disabled?`, `onClick?` |
| `Input` | `input` | `key`, `value?`, `revision?`, `placeholder?`, `disabled?`, `onChange?: (val: string) => void`, `onSubmit?: (val: string) => void` |
| `TextArea` | `textArea` | `key`, `value?`, `revision?`, `rows?` *(default 4)*, `placeholder?`, `disabled?`, `onChange?: (val: string) => void` |
| `Select` | `select` | `key`, `value?`, `options: Array<{ label: string; value: string }>`, `disabled?`, `onChange?: (val: string) => void` |
| `Checkbox` | `checkbox` | `key?`, `checked`, `label?`, `disabled?`, `onChange?: (checked: boolean) => void` |
| `Switch` | `switch` | `key?`, `checked`, `label?`, `disabled?`, `onChange?: (checked: boolean) => void` |
| `SegmentedTabs` | `segmentedTabs` | `key?`, `value`, `options: Array<{ label: string; value: string }>`, `onChange?: (val: string) => void` |
| `ListItem` | `listItem` | `key`, `title`, `subtitle?`, `badge?`, `trailing?`, `selected?`, `onClick?` |

---

## Host API Reference (`fern`)

Import via `import fern from "fern"` (or named imports `import { tasks, storage, ui } from "fern"`). Full TypeScript signatures live in [`sdk/fern-sdk/fern.d.ts`](../../sdk/fern-sdk/fern.d.ts).

| Namespace | Methods | Notes / Permission |
| --- | --- | --- |
| `fern.tasks` | `list(filter?)`, `get(id)`, `create(draft)`, `update(id, patch)`, `complete(id)`, `reopen(id)`, `delete(id)`, `subtasks(id)`, `addSubtask(parentId, title)`, `checklist(id)` | `update(id, patch)` also patches `checklist` items (`Array<string \| { title, completed? }>`) and nullable `scheduleBucket`. `complete(id)` runs canonical recurrence & subtask cascade. `tasks:read` / `tasks:write`. |
| `fern.lists` | `all()`, `folders()`, `sections(listId?)`, `create(draft)`, `rename(id, name)` | `create(draft)` accepts a name or `{ name, color?, icon?, folderId?, kind?, inSmartLists?, notes?, deadline? }`. `lists:read` / `lists:write`. |
| `fern.tags` | `all()`, `setTaskTags(taskId, tags)` | `tags:read` / `tags:write`. |
| `fern.habits` | `all()`, `stats(id)`, `checkin(id, date, status, amount?)` | `status`: `"done" \| "undone" \| "skip" \| "reset" \| "toggle" \| boolean`. `habits:read` / `habits:write`. |
| `fern.focus` | `sessions(range?)`, `stats(range?)` | `range`: `{ from?: "YYYY-MM-DD" \| ms, to?: "YYYY-MM-DD" \| ms }`. `focus:read`. |
| `fern.countdowns` | `all()`, `create({ name, date, kind? })`, `delete(id)` | `countdowns:read` / `countdowns:write`. |
| `fern.batch` | `batch(fn)` | Runs synchronous `fn()` inside one SQLite transaction; rolls back cleanly on throw and emits a single UI refresh. Callback must be synchronous. |
| `fern.storage` | `get(key)`, `set(key, val)`, `delete(key)`, `keys(prefix?)` | Isolated per-plugin JSON KV in SQLite (5 MiB quota). `storage`. |
| `fern.meta` | `get(type, id, key)`, `set(type, id, key, val)`, `list(type, id)`, `delete(type, id, key)` | `type`: `"task" \| "list" \| "habit" \| "countdown" \| "day"`, 64 KiB cap per entry. `meta`. |
| `fern.settings` | `get(key)`, `all()`, `hasSecret(key)`, `onChange(cb)` | Always available. Reading a `"secret"` key throws; use `${secret:key}` in HTTP headers. |
| `fern.http` | `fetch(url, { method?, headers?, body?, timeoutMs? })` | Async. Returns `{ ok, status, url, headers, body, text(), json() }`. `network` / `network:local`. |
| `fern.oauth` | `authorize(opts)`, `refresh(opts)`, `clearTokens(keys?)` | Async zero-secret OAuth 2.0 loopback login & refresh (default redirect URI `http://127.0.0.1:18481/callback`). See details below. |
| `fern.events` | `on(kind, cb)` → `unsubscribe()` | Kinds: `"task.created"`, `"task.updated"`, `"task.completed"`, `"task.reopened"`, `"task.deleted"`, `"task.*"`, `"habit.checkin"`, `"focus.completed"`, `"app.ready"`, `"*"`. Own writes are echo-suppressed; causal cascades stop at `MAX_EVENT_DEPTH = 4`. |
| `fern.hooks` | `beforeTaskCreate(cb)`, `beforeTaskComplete(cb)`, `quickAddTransform(cb)`, `onReminder(cb)` | `hooks:intercept`. Return `false` or `{ cancel: true, reason }` to cancel, `{ draft }` / `{ value }` or modified object to mutate, or `undefined`/`null` to allow unchanged. |
| `fern.timers` | `setInterval(id, ms, cb)`, `clearInterval(id)` | `ms >= 10_000` (10 seconds), max `8` active timers per plugin. |
| `fern.commands` | `register(id, (arg?) => result)` | Invoked via Command Search or `fern://plugin/<id>/<command>?...`. |
| `fern.search` | `registerProvider(id, (query) => SearchHit[])` | Returns hits `{ id?, title, subtitle?, badge?, taskId?, commandId?, commandArg? }`. |
| `fern.services` | `provide(name, fn)`, `call(name, arg?)` | Async inter-plugin RPC (max call depth `4`). `services`. |
| `fern.views` | `register(viewId, { render, mount?, unmount? })`, `refresh(viewId)` | `viewId` must be declared in `manifest.contributions.sidebar` or `manifest.contributions.pages`. `refresh` triggers a live re-render from timers, events, or async tasks. |
| `fern.ui` | `toast(msg, level?)`, `confirm(title, body)`, `prompt(title, placeholder?, defaultValue?)`, `openModal(title, viewOrNode)`, `closeModal()`, `openTask(taskId)`, `openSettings()` | `level`: `"info" \| "success" \| "warning" \| "error"`. `confirm`/`prompt` return Promises. `openSettings()` opens Settings directly to this plugin's settings card. |
| `fern.system` | `locale()`, `themeAppearance()`, `openExternal(url)`, `clipboardWrite(text)` | `openExternal` allows only `http://` / `https://`. `clipboardWrite` requires `clipboard`. |
| `console` | `log`, `info`, `warn`, `error`, `debug` | Ring buffer of 200 log entries per plugin; secret values are scrubbed automatically. |

### `fern.oauth` (Zero-Secret OAuth 2.0 Loopback)

`fern.oauth` runs the OAuth 2.0 Authorization Code loopback flow and token refresh entirely inside Rust so raw `client_secret`, `access_token`, and `refresh_token` values are never exposed to plugin JS (use `Authorization: Bearer ${secret:access_token}` in `fern.http.fetch`):

- **Default redirect URI:** `http://127.0.0.1:18481/callback` (`redirectHost: "127.0.0.1"`, `redirectPort: 18481`, `redirectPath: "/callback"`). Register this URI with your OAuth provider unless overriding `redirectHost` (`"127.0.0.1"` or `"localhost"`), `redirectPort` (`1024..=65535`), or `redirectPath`.
- **Permissions:** Requires `{ "network": [...] }` allowing the `authorizeUrl` and `tokenUrl` hosts (remote OAuth URLs must use `https://`; `http://` is permitted only on loopback with `"network:local"`). **`"network:local"` is NOT required** for the built-in loopback callback server.
- **Options & defaults:**
  - `tokenAuth`: `"basic"` *(default, HTTP Basic header)* or `"body"` *(form-encoded `client_id`/`client_secret`)*.
  - `pkce`: optional boolean *(default `false`; set `true` to send an S256 `code_challenge` and `code_verifier`)*.
  - `timeoutSec`: browser callback timeout in seconds, clamped to `5..=300` *(default `180`)*.
  - `saveAccessTokenAs` *(default `"access_token"`)*, `saveRefreshTokenAs` *(default `"refresh_token"`, pass `null` to skip)*, `refreshTokenKey` *(default `"refresh_token"`)*.
  - `clearTokens(keys?)`: removes stored token keys from `SecretsVault` *(defaults to `["access_token", "refresh_token"]`)* and returns the count removed.

---

## Script Runner Quick Recipes

For quick personal automations without creating a plugin directory or `manifest.json`, open **Settings → Plugins → Scripts**.

Scripts execute in a fresh sandboxed QuickJS runtime with `fern` exposed globally (`fern.*`). You can write:
- An ES module exporting `export default function(event) { ... }` or `export default async function(event) { ... }` *(recommended for event scripts and scripts using early `return;`)*, **or**
- Top-level statements (wrapped in an async function when using `await` or `return <value>`).

> [!NOTE]
> **Triggers & permissions in Scripts:** Currently, **manual** scripts (triggered via **Run** in Settings → Plugins → Scripts) and **event** scripts (dispatched automatically on matching domain events such as `task.created` or `task.completed`) execute at runtime. `command` and `interval` appear in the editor trigger selector for future dispatch; if you need Command Search commands, scheduled timers, `network` allowlists, or encrypted secrets today, create a plugin directory with `manifest.json`.

### 1. Auto-Tag New Tasks (`trigger: event`, `trigger_arg: task.created`)

Permissions: `["tasks:read", "tasks:write"]`

```js
export default function (event) {
  const task = event?.entity;
  const tags = Array.isArray(task?.tags) ? task.tags : [];
  if (!task || tags.includes("triage")) return;

  if (task.title.toLowerCase().includes("urgent")) {
    fern.tasks.update(task.id, {
      priority: "high",
      tags: [...tags, "triage"],
    });
    console.log("Auto-prioritized:", task.title);
  }
}
```

### 2. Move Overdue Tasks to Today (`trigger: manual`)

Permissions: `["tasks:read", "tasks:write"]`

```js
export default function () {
  const today = new Date().toISOString().slice(0, 10);
  const open = fern.tasks.list({ completed: false });
  const overdue = open.filter((t) => t.dueDate && t.dueDate < today);

  const moved = fern.batch(() => {
    for (const t of overdue) {
      fern.tasks.update(t.id, { dueDate: today });
    }
    return overdue.length;
  });

  fern.ui.toast(`Rescheduled ${moved} overdue task(s) to today`, "success");
  return { moved };
}
```

### 3. Copy Today's Summary as Markdown (`trigger: manual`)

Permissions: `["tasks:read", "clipboard"]`

```js
export default function () {
  const todayTasks = fern.tasks.list({ view: "today", completed: false });
  const lines = todayTasks.map((t) => `- [ ] ${t.title}${t.dueTime ? ` (${t.dueTime})` : ""}`);
  const md = lines.length ? `## Today\n${lines.join("\n")}` : "No open tasks for today.";

  fern.system.clipboardWrite(md);
  fern.ui.toast(`Copied ${lines.length} tasks to clipboard`, "info");
  return lines.length;
}
```

### 4. Interactive Goal Breakdown (`trigger: manual`)

Permissions: `["tasks:read", "tasks:write"]`

```js
export default async function () {
  const goal = await fern.ui.prompt("New goal title", "Launch v1.0 release");
  if (!goal || !goal.trim()) return null;

  const rawSteps = await fern.ui.prompt(
    "Comma-separated subtasks",
    "Write changelog, Run QA checks, Tag release",
  );
  const steps = (rawSteps ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);

  const parent = fern.batch(() => {
    const created = fern.tasks.create({ title: goal.trim() });
    for (const step of steps.slice(0, 10)) {
      fern.tasks.addSubtask(created.id, step);
    }
    return created;
  });

  fern.ui.openTask(parent.id);
  fern.ui.toast(`Created "${parent.title}" with ${steps.length} subtask(s)`, "success");
  return parent.id;
}
```

*(Need to call a local HTTP API such as Ollama at `http://127.0.0.1:11434`? Network permissions (`"network:local"` / `{ "network": [...] }`) are configured in a plugin's `manifest.json` rather than the flat Scripts permission chips.)*

---

## Security Model & Runtime Quotas

| Area | Enforcement in Fern |
| --- | --- |
| **Memory & stack** | `32 MiB` QuickJS heap limit, `512 KiB` max stack size per plugin runtime. |
| **Execution timeouts** | `2s` interrupt deadline for view renders, UI events, outbox events, timers, and hooks; `10s` for plugin activation, commands, search, services, and scripts. |
| **Circuit breaker** | `3` consecutive crashes or timeouts automatically disable the plugin (`CircuitBroken`) until reset in Settings. |
| **Module isolation** | Only `"fern"`, `"fern:ui"`, and canonicalized relative paths inside the plugin directory (<= `2 MiB` per file) may be imported. Symlink escapes and `..` traversal are rejected. |
| **Permission escalation** | Installing or updating a plugin with new permissions pauses activation until the user approves the permission diff. |
| **SSRF protection** | `fern.http.fetch` disables automatic redirects and manually follows up to `5` redirects, re-checking `network` allowlists and DNS-resolved IPs on every hop. Loopback (`127.0.0.1`, `::1`, `localhost`) requires `"network:local"`; private LAN targets require both `{"network": [...]}` and `"network:local"`; cloud metadata (`169.254.0.0/16`, `fe80::/10`) and IPv6 transition prefixes are always blocked. Max response size `10 MiB`, connect timeout `5s`, total timeout `20s`. |
| **Encrypted secrets** | Secrets live in `<FERN_DATA_DIR>/plugin-secrets.vault` (ChaCha20-Poly1305, separate from `tasks.db`, backups, and sync). Secret values are interpolated into HTTP headers via `${secret:key}`, stripped on cross-origin redirects, and scrubbed (`***`) from logs and error messages. |
| **SVG sanitization** | Icons and `<Svg>` nodes are capped at `256 KiB` and parsed strictly without DTDs, rejecting `<script>`, `<foreignObject>`, `<image>`, `<feImage>`, `<style>`, `on*` handlers, and external `href` URIs. |
| **Event loop guards** | Outbox events omit `originPlugin === self`, drop cascades at `MAX_EVENT_DEPTH = 4`, and never include `plugin_*` tables in E2EE sync. |
| **Archive safety** | `.zip` installs enforce `<= 10 MiB` compressed / `<= 32 MiB` unpacked limits, SHA-256 verification, symlink rejection, zip-slip protection, and atomic directory swaps. |

---

## AI-Authoring Checklist

When asking an AI assistant to generate or modify a Fern plugin:

1. **No DOM or Node APIs:** Never emit `window`, `document`, `localStorage`, global `fetch`, `process`, `fs`, or `setTimeout`. Use `@fern/sdk` JSX components, `fern.storage`, `fern.http.fetch`, and `fern.timers.setInterval(id, ms >= 10000, cb)`.
2. **Keep IDs & paths valid:** `manifest.id` must match `^[a-z0-9][a-z0-9._-]{1,63}$`, have no `..`, and match the folder name. Keep `entry` and icon paths relative, and keep view IDs unique across `contributions.sidebar` and `contributions.pages`.
3. **Declare minimal permissions:** Add only the permissions you call, plus `{ "network": ["host"] }` (and `"network:local"` for loopback/LAN; note `fern.oauth` does not need `"network:local"` for its loopback callback `http://127.0.0.1:18481/callback`).
4. **Never read secrets in JS:** Declare `{ "type": "secret" }` in `settings`, use `fern.oauth` for OAuth 2.0 tokens, and pass `"Authorization": "Bearer ${secret:key}"` in `fern.http.fetch` headers.
5. **Preserve input focus:** Always give `Input`, `TextArea`, `Select`, `Scroll`, and `ListItem` a stable `key`, and increment `Input`/`TextArea` `revision` only when resetting text from JS.
6. **Keep `fern.batch` synchronous:** Never pass an `async` callback to `fern.batch(() => ...)`.
7. **Bundle cleanly:** With TypeScript/JSX, bundle with `esbuild` marking `"fern"` and `"fern:ui"` as `external` into a single ESM file (`<= 2 MiB`).

---

## Publishing to `atabits/fern-plugins` & Custom Registries

Fern's **Browse** tab fetches `registry.json` (`schema_version: 1`) from `https://atabits.github.io/fern-plugins/registry.json` (plus any custom or self-hosted registry URLs configured by the user) with `If-None-Match` (`ETag`) conditional caching and offline cache fallback.

### Official Registry Workflow (`atabits/fern-plugins`)

1. Package your plugin as a `.zip` containing `manifest.json`, your entry JS (e.g. `index.js` or `dist/index.js`, `<= 2 MiB`), and any icons/assets (total `<= 10 MiB`, unpacked `<= 32 MiB`, no symlinks).
2. Publish a GitHub Release on your plugin repository with the `.zip` attached and compute its lowercase 64-char SHA-256 checksum.
3. Open a Pull Request in `atabits/fern-plugins` adding `plugins/<your-plugin-id>.json`:

```json
{
  "id": "com.example.github-issues",
  "version": "1.0.0",
  "source_url": "https://github.com/example/fern-github-issues/releases/download/v1.0.0/plugin.zip",
  "sha256": "<required-64-char-lowercase-hex>",
  "homepage": "https://github.com/example/fern-github-issues",
  "tags": ["sync", "developer"]
}
```

4. Drop [`docs/plugins/registry-workflow.yml`](./registry-workflow.yml) into `.github/workflows/registry.yml` in `atabits/fern-plugins`. On PRs it validates every descriptor (including required `sha256`), downloads changed archives with DNS-pinned HTTPS SSRF protection, and validates `manifest.json` (`sidebar` + `pages`, `commands`, `themes`, `locales`, `settings` labels/options, `<= 2 MiB` JS files); on `main` it verifies checksums on both new and existing releases, mirrors `plugin.zip` to GitHub Releases, and publishes `registry.json` + `plugin-descriptor.schema.json` to GitHub Pages.

### Self-Hosted Registries

Any static HTTPS server serving a `registry.json` matching `schema_version: 1` works as a custom registry source in **Settings → Plugins → Browse**:

```json
{
  "schema_version": 1,
  "generated_at": "2026-11-22T00:00:00Z",
  "plugins": [
    {
      "id": "com.example.github-issues",
      "name": "GitHub Issues",
      "version": "1.0.0",
      "api_version": 1,
      "min_fern_version": "0.2.0",
      "description": "Sync assigned GitHub issues and view them in a sidebar panel.",
      "author": "Your Name",
      "homepage": "https://github.com/example/fern-github-issues",
      "tags": ["sync", "developer"],
      "permissions": ["tasks:read", "tasks:write", { "network": ["api.github.com"] }],
      "download_url": "https://github.com/atabits/fern-plugins/releases/download/com.example.github-issues-1.0.0/plugin.zip",
      "sha256": "<64-hex-sha256>",
      "size_bytes": 18432
    }
  ]
}
```
