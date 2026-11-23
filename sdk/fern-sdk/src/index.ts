/// <reference path="../fern.d.ts" />
import fern from "fern";
import * as ui from "fern:ui";
import type { UiNode } from "fern:ui";
import {
  Fragment,
  uiBuilders,
  unwrapFragment,
  type BadgeProps,
  type ButtonProps,
  type CardProps,
  type CheckboxProps,
  type DividerProps,
  type DonutProps,
  type EmptyStateProps,
  type FernChild,
  type FernChildren,
  type FernTextChild,
  type FernTextChildren,
  type GridProps,
  type HeadingProps,
  type HStackProps,
  type IconButtonProps,
  type InputProps,
  type KeyValueProps,
  type ListItemProps,
  type MarkdownProps,
  type ProgressProps,
  type ScrollProps,
  type SectionProps,
  type SegmentedTabsProps,
  type SelectProps,
  type SpacerProps,
  type SvgProps,
  type SwitchProps,
  type TextAreaProps,
  type TextProps,
  type UiAlign,
  type UiButtonSize,
  type UiButtonVariant,
  type UiJustify,
  type UiSelectOption,
  type UiTextSize,
  type UiWeight,
  type VStackProps,
} from "./jsx-runtime";

export { Fragment };
export type {
  BadgeProps,
  ButtonProps,
  CardProps,
  CheckboxProps,
  DividerProps,
  DonutProps,
  EmptyStateProps,
  FernChild,
  FernChildren,
  FernTextChild,
  FernTextChildren,
  GridProps,
  HeadingProps,
  HStackProps,
  IconButtonProps,
  InputProps,
  KeyValueProps,
  ListItemProps,
  MarkdownProps,
  ProgressProps,
  ScrollProps,
  SectionProps,
  SegmentedTabsProps,
  SelectProps,
  SpacerProps,
  SvgProps,
  SwitchProps,
  TextAreaProps,
  TextProps,
  UiAlign,
  UiButtonSize,
  UiButtonVariant,
  UiJustify,
  UiNode,
  UiSelectOption,
  UiTextSize,
  UiWeight,
  VStackProps,
};

// ---------------------------------------------------------------------------
// Declarative UI Component Wrappers
// ---------------------------------------------------------------------------

export function VStack(props: VStackProps): UiNode {
  return uiBuilders.vstack(props as Record<string, unknown>);
}

export function HStack(props: HStackProps): UiNode {
  return uiBuilders.hstack(props as Record<string, unknown>);
}

export function Grid(props: GridProps): UiNode {
  return uiBuilders.grid(props as Record<string, unknown>);
}

export function Scroll(props: ScrollProps): UiNode {
  return uiBuilders.scroll(props as unknown as Record<string, unknown>);
}

export function Card(props: CardProps): UiNode {
  return uiBuilders.card(props as Record<string, unknown>);
}

export function Section(props: SectionProps): UiNode {
  return uiBuilders.section(props as unknown as Record<string, unknown>);
}

export function Divider(_props?: DividerProps): UiNode {
  return uiBuilders.divider();
}

export function Spacer(props?: SpacerProps): UiNode {
  return uiBuilders.spacer((props ?? {}) as Record<string, unknown>);
}

export function Heading(props: HeadingProps): UiNode {
  return uiBuilders.heading(props as Record<string, unknown>);
}

export function Text(props: TextProps): UiNode {
  return uiBuilders.text(props as Record<string, unknown>);
}

export function Markdown(props: MarkdownProps): UiNode {
  return uiBuilders.markdown(props as Record<string, unknown>);
}

export function KeyValue(props: KeyValueProps): UiNode {
  return uiBuilders.keyValue(props as unknown as Record<string, unknown>);
}

export function Badge(props: BadgeProps): UiNode {
  return uiBuilders.badge(props as Record<string, unknown>);
}

export function Progress(props: ProgressProps): UiNode {
  return uiBuilders.progress(props as unknown as Record<string, unknown>);
}

export function Donut(props: DonutProps): UiNode {
  return uiBuilders.donut(props as unknown as Record<string, unknown>);
}

export function EmptyState(props: EmptyStateProps): UiNode {
  return uiBuilders.emptyState(props as unknown as Record<string, unknown>);
}

export function Svg(props: SvgProps): UiNode {
  return uiBuilders.svg(props as unknown as Record<string, unknown>);
}

export function Button(props: ButtonProps): UiNode {
  return uiBuilders.button(props as Record<string, unknown>);
}

export function IconButton(props: IconButtonProps): UiNode {
  return uiBuilders.iconButton(props as unknown as Record<string, unknown>);
}

export function Input(props: InputProps): UiNode {
  return uiBuilders.input(props as unknown as Record<string, unknown>);
}

export function TextArea(props: TextAreaProps): UiNode {
  return uiBuilders.textArea(props as unknown as Record<string, unknown>);
}

export function Select(props: SelectProps): UiNode {
  return uiBuilders.select(props as unknown as Record<string, unknown>);
}

export function Checkbox(props: CheckboxProps): UiNode {
  return uiBuilders.checkbox(props as unknown as Record<string, unknown>);
}

export function Switch(props: SwitchProps): UiNode {
  return uiBuilders.switch(props as unknown as Record<string, unknown>);
}

export function SegmentedTabs(props: SegmentedTabsProps): UiNode {
  return uiBuilders.segmentedTabs(props as unknown as Record<string, unknown>);
}

export function ListItem(props: ListItemProps): UiNode {
  return uiBuilders.listItem(props as unknown as Record<string, unknown>);
}

// ---------------------------------------------------------------------------
// Lightweight QuickJS Reactive Hooks & View Definition
// ---------------------------------------------------------------------------

const MAX_RENDER_PASSES = 25;

type CleanupFn = () => void;
export type SetStateAction<T> = T | ((prev: T) => T);
export type StateSetter<T> = (action: SetStateAction<T>) => void;
export type EffectCallback = () => void | CleanupFn;
export type ViewRenderFn = () => UiNode | boolean | null | undefined | void;

interface StateHookSlot<T = unknown> {
  kind: "state";
  value: T;
  setter: StateSetter<T>;
}

interface EffectHookSlot {
  kind: "effect";
  deps: readonly unknown[] | undefined;
  cleanup: CleanupFn | undefined;
}

interface RefHookSlot<T = unknown> {
  kind: "ref";
  ref: { current: T };
}

interface MemoHookSlot<T = unknown> {
  kind: "memo";
  deps: readonly unknown[];
  value: T;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnySlot = any;
type HookSlot = StateHookSlot<AnySlot> | EffectHookSlot | RefHookSlot<AnySlot> | MemoHookSlot<AnySlot>;

interface PendingEffect {
  slot: EffectHookSlot;
  effect: EffectCallback;
  deps: readonly unknown[] | undefined;
}

interface ViewHookContext {
  hooks: HookSlot[];
  hookIndex: number;
  pendingEffects: PendingEffect[];
  dirty: boolean;
  isRendering: boolean;
  isFlushingEffects: boolean;
  mounted: boolean;
  generation: number;
  onInvalidate?: () => void;
}

export interface DefineViewOptions {
  /** Optional hook invoked when state changes outside a synchronous render/effect pass. */
  onInvalidate?: () => void;
}

export interface ViewDefinition {
  render: () => UiNode;
  mount: () => void;
  unmount: () => void;
}

const hookContextStack: ViewHookContext[] = [];

function currentContext(hookName: string): ViewHookContext {
  const ctx = hookContextStack[hookContextStack.length - 1];
  if (!ctx) {
    throw new Error(
      `${hookName}() must be called synchronously inside a component wrapped by defineView().`,
    );
  }
  return ctx;
}

function depsEqual(
  prev: readonly unknown[] | undefined,
  next: readonly unknown[] | undefined,
): boolean {
  if (prev === undefined || next === undefined || prev.length !== next.length) {
    return false;
  }
  for (let i = 0; i < prev.length; i += 1) {
    if (!Object.is(prev[i], next[i])) {
      return false;
    }
  }
  return true;
}

function claimSlot<S extends HookSlot>(
  ctx: ViewHookContext,
  expectedKind: S["kind"],
  init: () => S,
): S {
  const idx = ctx.hookIndex;
  ctx.hookIndex += 1;
  if (idx >= ctx.hooks.length) {
    const slot = init();
    ctx.hooks.push(slot);
    return slot;
  }
  const existing = ctx.hooks[idx];
  if (existing.kind !== expectedKind) {
    throw new Error(
      `Hook order changed between renders: expected "${existing.kind}" at slot ${idx}, got "${expectedKind}". Do not call hooks conditionally or inside loops.`,
    );
  }
  return existing as S;
}

/**
 * Stores reactive component state across QuickJS view renders.
 * Calling the returned setter inside a UI event handler updates the state slot
 * before Fern's host automatically re-invokes `render()`; calling it inside a
 * synchronous `useEffect` triggers an immediate re-render pass in `defineView`.
 */
export function useState<T>(initialValue: T | (() => T)): [T, StateSetter<T>] {
  const ctx = currentContext("useState");
  const slot = claimSlot<StateHookSlot<T>>(ctx, "state", () => {
    const slotGeneration = ctx.generation;
    const resolved =
      typeof initialValue === "function"
        ? (initialValue as () => T)()
        : initialValue;
    const created: StateHookSlot<T> = {
      kind: "state",
      value: resolved,
      setter: (action: SetStateAction<T>) => {
        if (!ctx.mounted || ctx.generation !== slotGeneration) {
          return;
        }
        const next =
          typeof action === "function"
            ? (action as (prev: T) => T)(created.value)
            : action;
        if (Object.is(created.value, next)) {
          return;
        }
        created.value = next;
        ctx.dirty = true;
        if (!ctx.isRendering && !ctx.isFlushingEffects) {
          ctx.onInvalidate?.();
        }
      },
    };
    return created;
  });
  return [slot.value, slot.setter];
}

/**
 * Runs a side-effect after render commit when `deps` change (or once on mount if `deps` is `[]`).
 * Any returned cleanup function runs before the next effect execution and on `unmount()`.
 */
export function useEffect(effect: EffectCallback, deps?: readonly unknown[]): void {
  const ctx = currentContext("useEffect");
  const isFirst = ctx.hookIndex >= ctx.hooks.length;
  const slot = claimSlot<EffectHookSlot>(ctx, "effect", () => ({
    kind: "effect",
    deps: undefined,
    cleanup: undefined,
  }));

  if (isFirst || !depsEqual(slot.deps, deps)) {
    ctx.pendingEffects.push({ slot, effect, deps });
  }
}

/** Returns a mutable `{ current }` container that persists across renders without triggering re-renders. */
export function useRef<T>(initialValue: T): { current: T } {
  const ctx = currentContext("useRef");
  const slot = claimSlot<RefHookSlot<T>>(ctx, "ref", () => ({
    kind: "ref",
    ref: { current: initialValue },
  }));
  return slot.ref;
}

/** Memoizes a computed value across renders until any dependency in `deps` changes. */
export function useMemo<T>(factory: () => T, deps: readonly unknown[]): T {
  const ctx = currentContext("useMemo");
  const isFirst = ctx.hookIndex >= ctx.hooks.length;
  const slot = claimSlot<MemoHookSlot<T>>(ctx, "memo", () => ({
    kind: "memo",
    deps,
    value: factory(),
  }));

  if (!isFirst && !depsEqual(slot.deps, deps)) {
    slot.deps = deps;
    slot.value = factory();
  }
  return slot.value;
}

/** Memoizes a callback reference across renders until any dependency in `deps` changes. */
export function useCallback<T extends (...args: never[]) => unknown>(
  callback: T,
  deps: readonly unknown[],
): T {
  return useMemo(() => callback, deps);
}

function finalizeRootNode(raw: UiNode | boolean | null | undefined | void): UiNode {
  if (!raw || typeof raw !== "object") {
    return ui.vstack({ gap: 0 }, []);
  }
  const fragChildren = unwrapFragment(raw);
  if (fragChildren) {
    if (fragChildren.length === 0) {
      return ui.vstack({ gap: 0 }, []);
    }
    if (fragChildren.length === 1) {
      return fragChildren[0];
    }
    return ui.vstack({ gap: 8 }, fragChildren);
  }
  return raw;
}

function cleanupAllEffects(ctx: ViewHookContext): void {
  let firstError: unknown;
  let hasError = false;
  for (const slot of ctx.hooks) {
    if (slot.kind === "effect" && slot.cleanup) {
      const cleanup = slot.cleanup;
      slot.cleanup = undefined;
      try {
        cleanup();
      } catch (err) {
        if (!hasError) {
          firstError = err;
          hasError = true;
        }
      }
    }
  }
  if (hasError) {
    throw firstError;
  }
}

/**
 * Wraps a functional JSX component using `useState` / `useEffect` into a Fern view object
 * (`{ render, mount, unmount }`) for `fern.views.register(...)` (sidebar views and plugin pages).
 *
 * Note: `fern.ui.openModal(...)` renders a one-shot `UiNode` tree and does not drive `mount()` /
 * `unmount()` or automatic re-renders. For reactive modals with proper `useEffect` cleanup,
 * use `openModalView(...)` / `closeModalView()`.
 */
export function defineView(
  component: ViewRenderFn,
  options?: DefineViewOptions,
): ViewDefinition {
  const ctx: ViewHookContext = {
    hooks: [],
    hookIndex: 0,
    pendingEffects: [],
    dirty: false,
    isRendering: false,
    isFlushingEffects: false,
    mounted: false,
    generation: 0,
    onInvalidate: options?.onInvalidate,
  };

  return {
    mount() {
      ctx.mounted = false;
      ctx.generation += 1;
      try {
        cleanupAllEffects(ctx);
      } finally {
        ctx.hooks = [];
        ctx.pendingEffects = [];
        ctx.dirty = false;
        ctx.mounted = true;
      }
    },

    render(): UiNode {
      if (!ctx.mounted) {
        ctx.generation += 1;
        ctx.mounted = true;
      }
      let result: UiNode | boolean | null | undefined | void;

      for (let pass = 0; pass < MAX_RENDER_PASSES; pass += 1) {
        ctx.dirty = false;
        ctx.hookIndex = 0;
        ctx.pendingEffects = [];
        ctx.isRendering = true;
        hookContextStack.push(ctx);
        try {
          result = component();
        } finally {
          hookContextStack.pop();
          ctx.isRendering = false;
        }

        const effects = ctx.pendingEffects;
        ctx.pendingEffects = [];
        if (effects.length > 0) {
          ctx.isFlushingEffects = true;
          try {
            for (const item of effects) {
              if (!ctx.mounted) {
                break;
              }
              if (item.slot.cleanup) {
                const prevCleanup = item.slot.cleanup;
                item.slot.cleanup = undefined;
                prevCleanup();
              }
              const nextCleanup = item.effect();
              item.slot.deps = item.deps;
              if (typeof nextCleanup === "function") {
                if (ctx.mounted) {
                  item.slot.cleanup = nextCleanup;
                } else {
                  nextCleanup();
                }
              }
            }
          } finally {
            ctx.isFlushingEffects = false;
          }
        }

        if (!ctx.dirty || !ctx.mounted) {
          return finalizeRootNode(result);
        }
      }

      throw new Error(
        `Too many re-renders in defineView (exceeded ${MAX_RENDER_PASSES} passes). Check for unconditional state updates during render or useEffect.`,
      );
    },

    unmount() {
      ctx.mounted = false;
      ctx.generation += 1;
      try {
        cleanupAllEffects(ctx);
      } finally {
        ctx.hooks = [];
        ctx.pendingEffects = [];
        ctx.dirty = false;
      }
    },
  };
}

export interface ModalControls {
  /** Unmounts the modal's hooks/effects and closes the host modal. */
  close: () => void;
}

export interface ModalHandle {
  /** Re-renders and pushes the latest modal tree to the host. */
  refresh: () => void;
  /** Unmounts the modal's hooks/effects and closes the host modal. */
  close: () => void;
}

let activeModalSession: { close: (notifyHost?: boolean) => void } | null = null;

/**
 * Closes any modal opened with `openModalView()`, running all `useEffect` cleanups
 * before dismissing the host modal.
 */
export function closeModalView(): void {
  if (activeModalSession) {
    activeModalSession.close(true);
  } else {
    fern.ui.closeModal();
  }
}

/**
 * Opens a reactive modal backed by `useState` / `useEffect` hooks.
 *
 * State updates automatically re-push the updated tree via `fern.ui.openModal(...)` so the
 * modal never stays stale, and calling `controls.close()`, `closeModalView()`, or opening
 * another modal automatically runs all `useEffect` cleanups so subscriptions never leak.
 */
export function openModalView(
  title: string,
  component: (controls: ModalControls) => UiNode | boolean | null | undefined | void,
): ModalHandle {
  if (activeModalSession) {
    activeModalSession.close(false);
  }

  let closed = false;
  let view!: ViewDefinition;

  const pushRender = (): void => {
    if (closed) {
      return;
    }
    const tree = view.render();
    if (!closed) {
      fern.ui.openModal(title, tree);
    }
  };

  const close = (notifyHost = true): void => {
    if (closed) {
      return;
    }
    closed = true;
    if (activeModalSession === session) {
      activeModalSession = null;
    }
    try {
      view.unmount();
    } finally {
      if (notifyHost) {
        fern.ui.closeModal();
      }
    }
  };

  const controls: ModalControls = { close: () => close(true) };
  view = defineView(() => component(controls), {
    onInvalidate: () => {
      pushRender();
    },
  });

  const session = { close };
  activeModalSession = session;

  try {
    view.mount();
    pushRender();
  } catch (err) {
    close(false);
    throw err;
  }

  return {
    refresh: pushRender,
    close: () => close(true),
  };
}
