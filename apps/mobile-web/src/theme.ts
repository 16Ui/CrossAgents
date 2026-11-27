// Theme system aligned 1:1 with desktop CrossAgents (`apps/gui/src/lib/theme.ts`).
// Supports 4 visual palettes (macOS 蓝 / 墨 · shadcn / 纸 / 雾), 3 modes (跟随系统 / 浅色 / 深色),
// and 3 prose font styles (默认无衬线 / 柔和 · 苹方 / 衬线 · Claude), while dynamically syncing
// `<meta name="theme-color">` so the iOS status bar blends seamlessly into the top bar.

import { useSyncExternalStore } from "react";

export type Mode = "system" | "light" | "dark";
export type Paper = "plain" | "ink" | "paper" | "mist";
export type FontStyle = "sans" | "soft" | "serif";
export type ThemeChoice = { mode: Mode; paper: Paper; font: FontStyle };
export type AppliedTheme = "light" | "light-paper" | "light-mist" | "dark";

export const MODES: { key: Mode; label: string }[] = [
  { key: "system", label: "跟随系统" },
  { key: "light", label: "浅色" },
  { key: "dark", label: "深色" },
];

export const PAPERS: { key: Paper; label: string; hint: string; bg: string; accent: string }[] = [
  { key: "plain", label: "macOS 蓝", hint: "中性灰白与系统蓝", bg: "#f5f5f7", accent: "#007aff" },
  { key: "ink", label: "墨 · shadcn", hint: "Zinc 极简黑白灰", bg: "#fafafa", accent: "#18181b" },
  { key: "paper", label: "纸", hint: "偏暖，像纸", bg: "#faf9f5", accent: "#007aff" },
  { key: "mist", label: "雾", hint: "偏冷的灰", bg: "#f3f4f6", accent: "#007aff" },
];

export const FONTS: { key: FontStyle; label: string }[] = [
  { key: "sans", label: "无衬线" },
  { key: "soft", label: "柔和" },
  { key: "serif", label: "衬线" },
];

const KEY = "crossagents.mobile.theme.v1";
const DEFAULT: ThemeChoice = { mode: "system", paper: "plain", font: "sans" };
const listeners = new Set<() => void>();
let choice: ThemeChoice = load();
let started = false;

function load(): ThemeChoice {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<ThemeChoice> | null;
    const mode = MODES.some((m) => m.key === saved?.mode) ? (saved!.mode as Mode) : DEFAULT.mode;
    const paper = PAPERS.some((p) => p.key === saved?.paper) ? (saved!.paper as Paper) : DEFAULT.paper;
    const font = FONTS.some((f) => f.key === saved?.font) ? (saved!.font as FontStyle) : DEFAULT.font;
    return { mode, paper, font };
  } catch {
    return DEFAULT;
  }
}

function isSystemDark(): boolean {
  return typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches;
}

export function resolveTheme(c: ThemeChoice, systemDark: boolean): AppliedTheme {
  const dark = c.mode === "dark" || (c.mode === "system" && systemDark);
  if (dark) return "dark";
  return c.paper === "plain" || c.paper === "ink" ? "light" : `light-${c.paper}`;
}

function syncMetaThemeColor() {
  const css = getComputedStyle(document.documentElement);
  const pageBg = css.getPropertyValue("--page").trim() || "#111113";
  let meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"][data-dynamic="1"]');
  if (!meta) {
    // Remove static media-scoped theme-color tags once dynamic control takes over so iOS respects
    // manual light/dark overrides.
    document.querySelectorAll('meta[name="theme-color"]').forEach((el) => el.remove());
    meta = document.createElement("meta");
    meta.name = "theme-color";
    meta.setAttribute("data-dynamic", "1");
    document.head.appendChild(meta);
  }
  meta.content = pageBg;
}

function apply() {
  const applied = resolveTheme(choice, isSystemDark());
  document.documentElement.dataset.theme = applied;
  if (choice.paper === "ink") document.documentElement.dataset.accent = "ink";
  else delete document.documentElement.dataset.accent;
  if (choice.font && choice.font !== "sans") document.documentElement.dataset.font = choice.font;
  else delete document.documentElement.dataset.font;
  syncMetaThemeColor();
}

export function startTheme() {
  if (started) return;
  started = true;
  apply();
  matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", () => {
    if (choice.mode === "system") apply();
  });
}

export function setTheme(next: Partial<ThemeChoice>) {
  choice = { ...choice, ...next };
  try {
    localStorage.setItem(KEY, JSON.stringify(choice));
  } catch {
    // ignore storage quota errors
  }
  apply();
  listeners.forEach((l) => l());
}

export function useTheme(): ThemeChoice {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => choice,
  );
}
