declare module "fern" {
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
    update(id: string, patch: Partial<TaskDraft> & { completed?: boolean; pinned?: boolean; inProgress?: boolean; wontDo?: boolean; parentId?: string | null }): Task;
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
    sections(listId?: string): Array<{ id: string; listId: string | null; name: string }>;
    create(draft: string | { name: string; color?: string; icon?: string; folderId?: string; notes?: string; deadline?: string }): TaskList;
    rename(id: string, name: string): boolean;
  };

  export const tags: {
    all(): Array<{ name: string; color: string | null; openCount: number; parent: string | null; pinned: boolean }>;
    setTaskTags(taskId: string, tags: string[]): boolean;
  };

  export const habits: {
    all(): Array<{ id: string; name: string; icon: string | null; color: string | null; startDate: string | null; goalDays: number | null; archived: boolean; goal: { amount: number; unit: string } | null; section: string }>;
    stats(id: string): { totalDays: number; streak: number; bestStreak: number };
    checkin(id: string, date: string, status: "done" | "undone" | "skip" | "reset" | "toggle" | boolean, amount?: number): boolean;
  };

  export const focus: {
    sessions(range?: { from?: string | number; to?: string | number }): Array<{ id: string; taskId: string | null; habitId: string | null; title: string; startedAt: number; endedAt: number; minutes: number; stopwatch: boolean; note: string }>;
    stats(range?: { from?: string | number; to?: string | number }): { todayCount: number; todayMinutes: number; totalCount: number; totalMinutes: number };
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
    keys(prefix?: string): string[];
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
    authorize(options: {
      authorizeUrl: string;
      tokenUrl: string;
      clientId: string;
      clientSecretKey?: string;
      scopes?: string[] | string;
      redirectHost?: "127.0.0.1" | "localhost";
      redirectPort?: number;
      redirectPath?: string;
      tokenAuth?: "basic" | "body";
      saveAccessTokenAs?: string;
      saveRefreshTokenAs?: string | null;
      extraAuthorizeParams?: Record<string, string>;
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
    clearTokens(keys?: string | string[]): number;
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
    register(viewId: string, view: { render: () => import("fern:ui").UiNode; mount?: () => void; unmount?: () => void }): void;
    refresh(viewId: string): void;
  };

  export const ui: {
    toast(message: string, level?: "info" | "success" | "warning" | "error"): void;
    confirm(title: string, body: string): Promise<boolean>;
    prompt(title: string, placeholder?: string, defaultValue?: string): Promise<string | null>;
    openModal(title: string, viewOrNode: import("fern:ui").UiNode | (() => import("fern:ui").UiNode) | { render: () => import("fern:ui").UiNode }): void;
    closeModal(): void;
    openTask(taskId: string): void;
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
  export function scroll(props: { key: string; maxHeight?: number }, children?: UiNode[]): UiNode;
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
  export function emptyState(props: { icon?: string; title: string; subtitle?: string; actionLabel?: string; onAction?: () => void }): UiNode;
  export function svg(props: { content: string; width?: number; height?: number }): UiNode;
  export function button(props: { label: string; variant?: "primary" | "secondary" | "destructive" | "ghost"; size?: "sm" | "md"; icon?: string; disabled?: boolean; loading?: boolean; onClick?: () => void }): UiNode;
  export function iconButton(props: { icon: string; tooltip?: string; disabled?: boolean; onClick?: () => void }): UiNode;
  export function input(props: { key: string; value: string; revision?: number; placeholder?: string; disabled?: boolean; onChange?: (value: string) => void; onSubmit?: (value: string) => void }): UiNode;
  export function textArea(props: { key: string; value: string; revision?: number; rows?: number; placeholder?: string; disabled?: boolean; onChange?: (value: string) => void }): UiNode;
  export function select(props: { key: string; value: string; options: Array<{ label: string; value: string }>; disabled?: boolean; onChange?: (value: string) => void }): UiNode;
  export function checkbox(props: { key?: string; checked: boolean; label?: string; disabled?: boolean; onChange?: (checked: boolean) => void }): UiNode;
  export function toggle(props: { key?: string; checked: boolean; label?: string; disabled?: boolean; onChange?: (checked: boolean) => void }): UiNode;
  export function segmentedTabs(props: { key?: string; value: string; options: Array<{ label: string; value: string }>; onChange?: (value: string) => void }): UiNode;
  export function listItem(props: { key: string; title: string; subtitle?: string; badge?: string; trailing?: string; selected?: boolean; onClick?: () => void }): UiNode;
}

interface Console {
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  debug(...args: unknown[]): void;
}

declare var console: Console;

declare namespace JSX {
  type Element = import("fern:ui").UiNode;
  type ElementType =
    | keyof IntrinsicElements
    | ((props: any) => Element | null | undefined | void);

  interface IntrinsicAttributes {
    key?: string | number;
  }

  interface ElementChildrenAttribute {
    children: {};
  }

  type FernAlign = "start" | "center" | "end" | "stretch";
  type FernJustify = "start" | "center" | "end" | "between";
  type FernWeight = "normal" | "medium" | "semibold" | "bold";
  type FernTextSize = "xs" | "sm" | "md" | "lg";
  type FernButtonVariant = "primary" | "secondary" | "destructive" | "ghost";
  type FernButtonSize = "sm" | "md";
  type FernSelectOption = { label: string; value: string };
  type FernChild = Element | string | number | boolean | null | undefined | FernChild[];
  type FernTextChild = string | number | boolean | null | undefined | FernTextChild[];

  interface IntrinsicElements {
    vstack: {
      gap?: number;
      padding?: number;
      align?: FernAlign;
      children?: FernChild;
    };
    hstack: {
      gap?: number;
      padding?: number;
      align?: FernAlign;
      justify?: FernJustify;
      wrap?: boolean;
      children?: FernChild;
    };
    grid: {
      cols?: number;
      gap?: number;
      children?: FernChild;
    };
    scroll: {
      key: string;
      maxHeight?: number;
      child?: Element;
      children?: FernChild;
    };
    card: {
      title?: string;
      subtitle?: string;
      padding?: number;
      children?: FernChild;
    };
    section: {
      title: string;
      subtitle?: string;
      children?: FernChild;
    };
    divider: Record<string, never>;
    spacer: {
      size?: number;
    };
    heading: {
      text?: string;
      level?: number;
      children?: FernTextChild;
    };
    text: {
      text?: string;
      muted?: boolean;
      weight?: FernWeight;
      color?: string;
      size?: FernTextSize;
      children?: FernTextChild;
    };
    markdown: {
      content?: string;
      children?: FernTextChild;
    };
    keyValue: {
      label: string;
      value: string;
      muted?: boolean;
    };
    key_value: {
      label: string;
      value: string;
      muted?: boolean;
    };
    badge: {
      label?: string;
      color?: string;
      children?: FernTextChild;
    };
    progress: {
      value: number;
      label?: string;
      color?: string;
    };
    donut: {
      value: number;
      label?: string;
      size?: number;
      color?: string;
    };
    emptyState: {
      icon?: string;
      title: string;
      subtitle?: string;
      actionLabel?: string;
      onAction?: () => void | Promise<void>;
    };
    empty_state: {
      icon?: string;
      title: string;
      subtitle?: string;
      actionLabel?: string;
      onAction?: () => void | Promise<void>;
    };
    svg: {
      content: string;
      width?: number;
      height?: number;
    };
    button: {
      label?: string;
      variant?: FernButtonVariant;
      size?: FernButtonSize;
      icon?: string;
      disabled?: boolean;
      loading?: boolean;
      onClick?: () => void | Promise<void>;
      children?: FernTextChild;
    };
    iconButton: {
      icon: string;
      tooltip?: string;
      disabled?: boolean;
      onClick?: () => void | Promise<void>;
    };
    icon_button: {
      icon: string;
      tooltip?: string;
      disabled?: boolean;
      onClick?: () => void | Promise<void>;
    };
    input: {
      key: string;
      value?: string;
      revision?: number;
      placeholder?: string;
      disabled?: boolean;
      onChange?: (value: string) => void | Promise<void>;
      onSubmit?: (value: string) => void | Promise<void>;
    };
    textArea: {
      key: string;
      value?: string;
      revision?: number;
      rows?: number;
      placeholder?: string;
      disabled?: boolean;
      onChange?: (value: string) => void | Promise<void>;
    };
    text_area: {
      key: string;
      value?: string;
      revision?: number;
      rows?: number;
      placeholder?: string;
      disabled?: boolean;
      onChange?: (value: string) => void | Promise<void>;
    };
    select: {
      key: string;
      value?: string;
      options: FernSelectOption[];
      disabled?: boolean;
      onChange?: (value: string) => void | Promise<void>;
    };
    checkbox: {
      key?: string;
      checked: boolean;
      label?: string;
      disabled?: boolean;
      onChange?: (checked: boolean) => void | Promise<void>;
    };
    switch: {
      key?: string;
      checked: boolean;
      label?: string;
      disabled?: boolean;
      onChange?: (checked: boolean) => void | Promise<void>;
    };
    toggle: {
      key?: string;
      checked: boolean;
      label?: string;
      disabled?: boolean;
      onChange?: (checked: boolean) => void | Promise<void>;
    };
    segmentedTabs: {
      key?: string;
      value: string;
      options: FernSelectOption[];
      onChange?: (value: string) => void | Promise<void>;
    };
    segmented_tabs: {
      key?: string;
      value: string;
      options: FernSelectOption[];
      onChange?: (value: string) => void | Promise<void>;
    };
    listItem: {
      key: string;
      title: string;
      subtitle?: string;
      badge?: string;
      trailing?: string;
      selected?: boolean;
      onClick?: () => void | Promise<void>;
    };
    list_item: {
      key: string;
      title: string;
      subtitle?: string;
      badge?: string;
      trailing?: string;
      selected?: boolean;
      onClick?: () => void | Promise<void>;
    };
  }
}
