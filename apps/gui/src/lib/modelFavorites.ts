import { useSyncExternalStore } from "react";

const STORAGE_KEY = "crossagents.modelFavorites";
const listeners = new Set<() => void>();
let cacheRaw: string | null | undefined;
let cacheSet: ReadonlySet<string> = new Set();

export function favoriteKey(plugin: string, model: string, account?: string): string {
  return `${plugin}:${model}${account ? `@${account}` : ""}`;
}

export function toggleFavoriteList(keys: readonly string[], key: string): string[] {
  return keys.includes(key) ? keys.filter((k) => k !== key) : [key, ...keys];
}

function readRaw(): string | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

function snapshot(): ReadonlySet<string> {
  const raw = readRaw();
  if (raw === cacheRaw) return cacheSet;
  cacheRaw = raw;
  try {
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    cacheSet = new Set(Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : []);
  } catch {
    cacheSet = new Set();
  }
  return cacheSet;
}

export function toggleFavorite(key: string): void {
  const next = toggleFavoriteList([...snapshot()], key);
  const serialized = JSON.stringify(next);
  try {
    localStorage.setItem(STORAGE_KEY, serialized);
  } catch {
    // Ignore storage quota / disabled storage.
  }
  cacheRaw = serialized;
  cacheSet = new Set(next);
  for (const fn of listeners) fn();
}

export function useModelFavorites(): ReadonlySet<string> {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    snapshot,
    snapshot,
  );
}
