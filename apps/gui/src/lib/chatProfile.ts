import { useSyncExternalStore } from "react";
import { api } from "../api";
import { onHubEvent } from "../hub";
import { reconcileChatProfile, type ChatProfile, type StoredChatProfile } from "./chatProfileSync";

export { reconcileChatProfile, type ChatProfile, type StoredChatProfile };

const KEY = "crossagents.chatProfile";
export const DEFAULT_CHAT_PROFILE: ChatProfile = { name: "", avatar: null, showAvatars: true };

function readStored(): StoredChatProfile {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return {
      name: typeof saved?.name === "string" ? saved.name.slice(0, 32) : "",
      avatar: typeof saved?.avatar === "string" && saved.avatar ? saved.avatar : null,
      showAvatars: saved?.showAvatars !== false,
      synced: saved?.synced === true,
    };
  } catch {
    return { ...DEFAULT_CHAT_PROFILE, synced: false };
  }
}

let stored: StoredChatProfile = readStored();
let profile: ChatProfile = { name: stored.name, avatar: stored.avatar, showAvatars: stored.showAvatars };
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((listener) => listener());
}

function writeLocal(next: StoredChatProfile) {
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Ignore storage quota or SSR errors.
  }
  stored = next;
  profile = { name: next.name, avatar: next.avatar, showAvatars: next.showAvatars };
  notify();
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (event.key !== KEY && event.key !== null) return;
    stored = readStored();
    profile = { name: stored.name, avatar: stored.avatar, showAvatars: stored.showAvatars };
    notify();
  });
}

export function setChatProfile(patch: Partial<ChatProfile>): Promise<void> {
  const touchesHub = patch.name !== undefined || patch.avatar !== undefined;
  const next: StoredChatProfile = {
    ...stored,
    ...patch,
    name: (patch.name ?? stored.name).slice(0, 32),
    synced: touchesHub ? false : stored.synced,
  };
  writeLocal(next);
  if (!touchesHub) return Promise.resolve();
  return api.setProfile({ name: next.name, avatar: next.avatar }).then((saved) => {
    const savedName = typeof saved?.name === "string" ? saved.name.slice(0, 32) : next.name;
    const savedAvatar = typeof saved?.avatar === "string" && saved.avatar ? saved.avatar : null;
    if (stored.name === next.name && stored.avatar === next.avatar) {
      writeLocal({ ...stored, name: savedName, avatar: savedAvatar, synced: true });
    }
  });
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
      const { next, push } = reconcileChatProfile(stored, remote);
      if (
        next.name !== stored.name
        || next.avatar !== stored.avatar
        || next.synced !== stored.synced
      ) {
        writeLocal(next);
      }
      if (push) {
        const saved = await api.setProfile(push);
        const savedName = typeof saved?.name === "string" ? saved.name.slice(0, 32) : push.name;
        const savedAvatar = typeof saved?.avatar === "string" && saved.avatar ? saved.avatar : null;
        if (stored.name === push.name && stored.avatar === push.avatar) {
          writeLocal({ ...stored, name: savedName, avatar: savedAvatar, synced: true });
        }
      }
    } catch {
      // Offline or older hub without /v1/profile: keep local cache.
    }
  };
  void sync(true);
  onHubEvent((event) => {
    if (event.type === "profile.updated") void sync(true);
  });
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
