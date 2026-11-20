import { useSyncExternalStore } from "react";

export type ChatProfile = {
  name: string;
  /** A square WebP, stored on this device. */
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

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (event.key !== KEY && event.key !== null) return;
    profile = readProfile();
    notify();
  });
}

export function setChatProfile(patch: Partial<ChatProfile>) {
  const next = { ...profile, ...patch };
  localStorage.setItem(KEY, JSON.stringify(next));
  profile = next;
  notify();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export const useChatProfile = () => useSyncExternalStore(subscribe, () => profile);
