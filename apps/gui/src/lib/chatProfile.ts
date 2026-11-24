import { useSyncExternalStore } from "react";
import { api } from "../api";

export type ChatProfile = {
  name: string;
  /** A square WebP, synced to the connected hub and cached locally. */
  avatar: string | null;
  showAvatars: boolean;
};

const KEY = "crossagents.chatProfile";
export const DEFAULT_CHAT_PROFILE: ChatProfile = { name: "", avatar: null, showAvatars: true };

function readProfile(): ChatProfile {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return {
      name: typeof saved?.name === "string" ? saved.name.slice(0, 32) : "",
      avatar: typeof saved?.avatar === "string" && saved.avatar ? saved.avatar : null,
      showAvatars: saved?.showAvatars !== false,
    };
  } catch {
    return DEFAULT_CHAT_PROFILE;
  }
}

let profile = readProfile();
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((listener) => listener());
}

function writeLocal(next: ChatProfile) {
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Ignore storage quota or SSR errors.
  }
  profile = next;
  notify();
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (event.key !== KEY && event.key !== null) return;
    profile = readProfile();
    notify();
  });
}

export function setChatProfile(patch: Partial<ChatProfile>) {
  const next: ChatProfile = {
    ...profile,
    ...patch,
    name: (patch.name ?? profile.name).slice(0, 32),
  };
  writeLocal(next);
  if (patch.name !== undefined || patch.avatar !== undefined) {
    void api.setProfile({ name: next.name, avatar: next.avatar }).catch(() => undefined);
  }
}

let syncStarted = false;
let lastSyncAt = 0;
export function startChatProfileSync() {
  if (syncStarted || typeof window === "undefined") return;
  syncStarted = true;
  const sync = async (force = false) => {
    const now = Date.now();
    if (!force && now - lastSyncAt < 5000) return;
    lastSyncAt = now;
    try {
      const remote = await api.profile();
      const remoteName = typeof remote?.name === "string" ? remote.name.slice(0, 32) : "";
      const remoteAvatar = typeof remote?.avatar === "string" && remote.avatar ? remote.avatar : null;
      if (remoteName || remoteAvatar) {
        if (remoteName !== profile.name || remoteAvatar !== profile.avatar) {
          writeLocal({ ...profile, name: remoteName, avatar: remoteAvatar });
        }
      } else if (profile.name || profile.avatar) {
        // First time on a hub without a saved profile: migrate this device's local profile up.
        await api.setProfile({ name: profile.name, avatar: profile.avatar });
      }
    } catch {
      // Offline or older hub without /v1/profile: keep local cache.
    }
  };
  void sync(true);
  window.crossagents?.onStatus((status) => {
    if (status === "connected") void sync(true);
  });
  window.addEventListener("focus", () => {
    void sync(false);
  });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export const useChatProfile = () => useSyncExternalStore(subscribe, () => profile);
