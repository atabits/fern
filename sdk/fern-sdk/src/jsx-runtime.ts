/// <reference path="../fern.d.ts" />
import * as ui from "fern:ui";
import type { UiNode } from "fern:ui";

export type { UiNode };

export type UiAlign = "start" | "center" | "end" | "stretch";
export type UiJustify = "start" | "center" | "end" | "between";
export type UiWeight = "normal" | "medium" | "semibold" | "bold";
export type UiTextSize = "xs" | "sm" | "md" | "lg";
export type UiButtonVariant = "primary" | "secondary" | "destructive" | "ghost";
export type UiButtonSize = "sm" | "md";

export interface UiSelectOption {
  label: string;
  value: string;
}

export type FernChild =
  | UiNode
  | string
  | number
  | boolean
  | null
  | undefined
  | FernChild[];

export type FernChildren = FernChild;

export type FernTextChild =
  | string
  | number
  | boolean
  | null
  | undefined
  | FernTextChild[];

export type FernTextChildren = FernTextChild;

export interface VStackProps {
  gap?: number;
  padding?: number;
  align?: UiAlign;
  children?: FernChildren;
}

export interface HStackProps {
  gap?: number;
  padding?: number;
  align?: UiAlign;
  justify?: UiJustify;
  wrap?: boolean;
  children?: FernChildren;
}

export interface GridProps {
  cols?: number;
  gap?: number;
  children?: FernChildren;
}

export interface ScrollProps {
  key: string;
  maxHeight?: number;
  child?: UiNode;
  children?: FernChildren;
}

export interface CardProps {
  title?: string;
  subtitle?: string;
  padding?: number;
  children?: FernChildren;
}

export interface SectionProps {
  title: string;
  subtitle?: string;
  children?: FernChildren;
}

export type DividerProps = Record<string, never>;

export interface SpacerProps {
  size?: number;
}

export interface HeadingProps {
  text?: string;
  level?: number;
  children?: FernTextChildren;
}

export interface TextProps {
  text?: string;
  muted?: boolean;
  weight?: UiWeight;
  color?: string;
  size?: UiTextSize;
  children?: FernTextChildren;
}

export interface MarkdownProps {
  content?: string;
  children?: FernTextChildren;
}

export interface KeyValueProps {
  label: string;
  value: string;
  muted?: boolean;
}

export interface BadgeProps {
  label?: string;
  color?: string;
  children?: FernTextChildren;
}

export interface ProgressProps {
  value: number;
  label?: string;
  color?: string;
}

export interface DonutProps {
  value: number;
  label?: string;
  size?: number;
  color?: string;
}

export interface EmptyStateProps {
  icon?: string;
  title: string;
  subtitle?: string;
  actionLabel?: string;
  onAction?: () => void | Promise<void>;
}

export interface SvgProps {
  content: string;
  width?: number;
  height?: number;
}

export interface ButtonProps {
  label?: string;
  variant?: UiButtonVariant;
  size?: UiButtonSize;
  icon?: string;
  disabled?: boolean;
  loading?: boolean;
  onClick?: () => void | Promise<void>;
  children?: FernTextChildren;
}

export interface IconButtonProps {
  icon: string;
  tooltip?: string;
  disabled?: boolean;
  onClick?: () => void | Promise<void>;
}

export interface InputProps {
  key: string;
  value?: string;
  revision?: number;
  placeholder?: string;
  disabled?: boolean;
  onChange?: (value: string) => void | Promise<void>;
  onSubmit?: (value: string) => void | Promise<void>;
}

export interface TextAreaProps {
  key: string;
  value?: string;
  revision?: number;
  rows?: number;
  placeholder?: string;
  disabled?: boolean;
  onChange?: (value: string) => void | Promise<void>;
}

export interface SelectProps {
  key: string;
  value?: string;
  options: UiSelectOption[];
  disabled?: boolean;
  onChange?: (value: string) => void | Promise<void>;
}

export interface CheckboxProps {
  key?: string;
  checked: boolean;
  label?: string;
  disabled?: boolean;
  onChange?: (checked: boolean) => void | Promise<void>;
}

export interface SwitchProps {
  key?: string;
  checked: boolean;
  label?: string;
  disabled?: boolean;
  onChange?: (checked: boolean) => void | Promise<void>;
}

export interface SegmentedTabsProps {
  key?: string;
  value: string;
  options: UiSelectOption[];
  onChange?: (value: string) => void | Promise<void>;
}

export interface ListItemProps {
  key: string;
  title: string;
  subtitle?: string;
  badge?: string;
  trailing?: string;
  selected?: boolean;
  onClick?: () => void | Promise<void>;
}

const FRAGMENT_REGISTRY_SYM = Symbol.for("@fern/sdk/fragmentRegistry");
const globalStore = globalThis as Record<symbol, WeakMap<object, UiNode[]> | undefined>;
const fragmentRegistry: WeakMap<object, UiNode[]> =
  globalStore[FRAGMENT_REGISTRY_SYM] ??
  (globalStore[FRAGMENT_REGISTRY_SYM] = new WeakMap<object, UiNode[]>());

/** Returns child nodes associated with a `<Fragment>` node, if any. */
export function unwrapFragment(node: UiNode): UiNode[] | undefined {
  return fragmentRegistry.get(node);
}

/**
 * Normalizes arbitrary JSX children into a flat list of `UiNode`s.
 * Flattens nested arrays and `<Fragment>`s, drops `null`/`undefined`/booleans,
 * and coalesces adjacent string/number primitives into `fern:ui` `text` nodes.
 */
export function normalizeChildren(children: FernChildren): UiNode[] {
  const out: UiNode[] = [];
  let textBuffer = "";
  let hasBufferedText = false;

  const flushText = (): void => {
    if (hasBufferedText) {
      out.push(ui.text({ text: textBuffer }));
      textBuffer = "";
      hasBufferedText = false;
    }
  };

  const visit = (item: FernChildren): void => {
    if (item === null || item === undefined || typeof item === "boolean") {
      return;
    }
    if (Array.isArray(item)) {
      for (const child of item) {
        visit(child);
      }
      return;
    }
    if (typeof item === "string" || typeof item === "number") {
      textBuffer += String(item);
      hasBufferedText = true;
      return;
    }
    if (typeof item === "object") {
      const fragChildren = fragmentRegistry.get(item);
      if (fragChildren) {
        flushText();
        out.push(...fragChildren);
        return;
      }
      flushText();
      out.push(item);
    }
  };

  visit(children);
  flushText();
  return out;
}

/** Joins text/number JSX children into a single string, returning `undefined` if none exist. */
export function joinTextChildren(children: FernTextChildren): string | undefined {
  let result = "";
  let seen = false;

  const visit = (item: FernTextChildren): void => {
    if (item === null || item === undefined || typeof item === "boolean") {
      return;
    }
    if (Array.isArray(item)) {
      for (const child of item) {
        visit(child);
      }
      return;
    }
    if (typeof item === "string" || typeof item === "number") {
      result += String(item);
      seen = true;
    }
  };

  visit(children);
  return seen ? result : undefined;
}

/** Copies only explicitly defined (`!== undefined`) allowlisted keys into a fresh object. */
function pickDefined(
  source: Record<string, unknown>,
  keys: readonly string[],
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of keys) {
    const val = source[key];
    if (val !== undefined) {
      out[key] = val;
    }
  }
  return out;
}

/** JSX Fragment (`<>...</>`). Unwraps cleanly inside parent containers and falls back to `vstack` at root. */
export function Fragment(props: { children?: FernChildren }): UiNode {
  const flat = normalizeChildren(props?.children);
  const node = ui.vstack({ gap: flat.length === 0 ? 0 : 8 }, flat);
  fragmentRegistry.set(node, flat);
  return node;
}

type NodeBuilder = (props: Record<string, unknown>) => UiNode;

function makeLayoutBuilder(
  fn: (props: Record<string, unknown>, children?: UiNode[]) => UiNode,
  allowedKeys: readonly string[],
): NodeBuilder {
  return (props) => {
    const clean = pickDefined(props, allowedKeys);
    const children = normalizeChildren(props.children as FernChildren);
    return fn(clean, children);
  };
}

function makeSimpleBuilder(
  fn: (props: never) => UiNode,
  allowedKeys: readonly string[],
): NodeBuilder {
  return (props) => fn(pickDefined(props, allowedKeys) as never);
}

function makeTextLikeBuilder(
  fn: (props: never) => UiNode,
  primaryKey: string,
  extraKeys: readonly string[],
): NodeBuilder {
  return (props) => {
    const clean = pickDefined(props, extraKeys);
    const explicit = props[primaryKey];
    const resolved =
      explicit !== undefined
        ? String(explicit)
        : (joinTextChildren(props.children as FernTextChildren) ?? "");
    clean[primaryKey] = resolved;
    return fn(clean as never);
  };
}

export const uiBuilders = {
  vstack: makeLayoutBuilder(ui.vstack, ["gap", "padding", "align"]),
  hstack: makeLayoutBuilder(ui.hstack, ["gap", "padding", "align", "justify", "wrap"]),
  grid: makeLayoutBuilder(ui.grid, ["cols", "gap"]),
  scroll: makeLayoutBuilder(
    ui.scroll as (props: Record<string, unknown>, children?: UiNode[]) => UiNode,
    ["key", "maxHeight", "child"],
  ),
  card: makeLayoutBuilder(ui.card, ["title", "subtitle", "padding"]),
  section: makeLayoutBuilder(
    ui.section as (props: Record<string, unknown>, children?: UiNode[]) => UiNode,
    ["title", "subtitle"],
  ),
  divider: (): UiNode => ui.divider(),
  spacer: (props: Record<string, unknown>): UiNode =>
    ui.spacer(props.size !== undefined ? { size: Number(props.size) } : undefined),
  heading: makeTextLikeBuilder(ui.heading, "text", ["level"]),
  text: makeTextLikeBuilder(ui.text, "text", ["muted", "weight", "color", "size"]),
  markdown: makeTextLikeBuilder(ui.markdown, "content", []),
  keyValue: makeSimpleBuilder(ui.keyValue, ["label", "value", "muted"]),
  badge: makeTextLikeBuilder(ui.badge, "label", ["color"]),
  progress: makeSimpleBuilder(ui.progress, ["value", "label", "color"]),
  donut: makeSimpleBuilder(ui.donut, ["value", "label", "size", "color"]),
  emptyState: makeSimpleBuilder(ui.emptyState, [
    "icon",
    "title",
    "subtitle",
    "actionLabel",
    "onAction",
  ]),
  svg: makeSimpleBuilder(ui.svg, ["content", "width", "height"]),
  button: makeTextLikeBuilder(ui.button, "label", [
    "variant",
    "size",
    "icon",
    "disabled",
    "loading",
    "onClick",
  ]),
  iconButton: makeSimpleBuilder(ui.iconButton, ["icon", "tooltip", "disabled", "onClick"]),
  input: (props: Record<string, unknown>): UiNode => {
    const clean = pickDefined(props, [
      "key",
      "value",
      "revision",
      "placeholder",
      "disabled",
      "onChange",
      "onSubmit",
    ]);
    if (clean.value === undefined) {
      clean.value = "";
    }
    return ui.input(clean as never);
  },
  textArea: (props: Record<string, unknown>): UiNode => {
    const clean = pickDefined(props, [
      "key",
      "value",
      "revision",
      "rows",
      "placeholder",
      "disabled",
      "onChange",
    ]);
    if (clean.value === undefined) {
      clean.value = "";
    }
    return ui.textArea(clean as never);
  },
  select: (props: Record<string, unknown>): UiNode => {
    const clean = pickDefined(props, ["key", "value", "options", "disabled", "onChange"]);
    if (clean.value === undefined) {
      clean.value = "";
    }
    return ui.select(clean as never);
  },
  checkbox: makeSimpleBuilder(ui.checkbox, ["key", "checked", "label", "disabled", "onChange"]),
  switch: makeSimpleBuilder(ui.toggle, ["key", "checked", "label", "disabled", "onChange"]),
  segmentedTabs: makeSimpleBuilder(ui.segmentedTabs, ["key", "value", "options", "onChange"]),
  listItem: makeSimpleBuilder(ui.listItem, [
    "key",
    "title",
    "subtitle",
    "badge",
    "trailing",
    "selected",
    "onClick",
  ]),
} as const;

const INTRINSIC_BUILDERS: Record<string, NodeBuilder> = {
  vstack: uiBuilders.vstack,
  hstack: uiBuilders.hstack,
  grid: uiBuilders.grid,
  scroll: uiBuilders.scroll,
  card: uiBuilders.card,
  section: uiBuilders.section,
  divider: uiBuilders.divider,
  spacer: uiBuilders.spacer,
  heading: uiBuilders.heading,
  text: uiBuilders.text,
  markdown: uiBuilders.markdown,
  keyValue: uiBuilders.keyValue,
  key_value: uiBuilders.keyValue,
  badge: uiBuilders.badge,
  progress: uiBuilders.progress,
  donut: uiBuilders.donut,
  emptyState: uiBuilders.emptyState,
  empty_state: uiBuilders.emptyState,
  svg: uiBuilders.svg,
  button: uiBuilders.button,
  iconButton: uiBuilders.iconButton,
  icon_button: uiBuilders.iconButton,
  input: uiBuilders.input,
  textArea: uiBuilders.textArea,
  text_area: uiBuilders.textArea,
  select: uiBuilders.select,
  checkbox: uiBuilders.checkbox,
  switch: uiBuilders.switch,
  toggle: uiBuilders.switch,
  segmentedTabs: uiBuilders.segmentedTabs,
  segmented_tabs: uiBuilders.segmentedTabs,
  listItem: uiBuilders.listItem,
  list_item: uiBuilders.listItem,
};

export type ComponentFunction<P = Record<string, unknown>> = (
  props: P,
) => UiNode | boolean | null | undefined | void;

export type ElementType = keyof JSX.IntrinsicElements | ComponentFunction<any>;

/** Automatic JSX factory (`jsx` / `jsxs`) compiling JSX elements directly into `fern:ui` `UiNode`s. */
export function jsx(
  type: ElementType | typeof Fragment,
  props: Record<string, unknown> | null | undefined,
  key?: string | number,
): UiNode {
  const rawProps = props ?? {};
  const mergedProps =
    key !== undefined && key !== null && rawProps.key === undefined
      ? { ...rawProps, key: String(key) }
      : rawProps;

  if (typeof type === "function") {
    const rendered = type(mergedProps);
    if (rendered === null || rendered === undefined || typeof rendered === "boolean") {
      return Fragment({ children: [] });
    }
    return rendered;
  }

  if (typeof type === "string") {
    const builder = INTRINSIC_BUILDERS[type];
    if (!builder) {
      throw new Error(
        `Unknown Fern UI element "<${type}>". Fern QuickJS plugins have no browser DOM; use Fern components such as <VStack>, <Text>, or <Button>.`,
      );
    }
    return builder(mergedProps);
  }

  throw new Error(`Unsupported JSX element type: ${String(type)}`);
}

export const jsxs = jsx;
export const jsxDEV = jsx;

export namespace JSX {
  export type Element = UiNode;
  export type ElementType = keyof IntrinsicElements | ComponentFunction<any>;

  export interface IntrinsicAttributes {
    key?: string | number;
  }

  export interface ElementChildrenAttribute {
    children: {};
  }

  export interface IntrinsicElements {
    vstack: VStackProps;
    hstack: HStackProps;
    grid: GridProps;
    scroll: ScrollProps;
    card: CardProps;
    section: SectionProps;
    divider: DividerProps;
    spacer: SpacerProps;
    heading: HeadingProps;
    text: TextProps;
    markdown: MarkdownProps;
    keyValue: KeyValueProps;
    key_value: KeyValueProps;
    badge: BadgeProps;
    progress: ProgressProps;
    donut: DonutProps;
    emptyState: EmptyStateProps;
    empty_state: EmptyStateProps;
    svg: SvgProps;
    button: ButtonProps;
    iconButton: IconButtonProps;
    icon_button: IconButtonProps;
    input: InputProps;
    textArea: TextAreaProps;
    text_area: TextAreaProps;
    select: SelectProps;
    checkbox: CheckboxProps;
    switch: SwitchProps;
    toggle: SwitchProps;
    segmentedTabs: SegmentedTabsProps;
    segmented_tabs: SegmentedTabsProps;
    listItem: ListItemProps;
    list_item: ListItemProps;
  }
}
