const KEYS = [
  "crossagents.theme", "crossagents.chatProfile", "crossagents.panes",
  "crossagents.widths", "crossagents.sidebarHidden", "crossagents.railHidden",
  "crossagents.railFolded", "crossagents.railOpened",
] as const;

export const PREFERENCES_MAX_BYTES = 2 * 1024 * 1024;

export function exportPreferences(storage: Storage): string {
  const settings = Object.fromEntries(KEYS.flatMap((key) => {
    const value = storage.getItem(key);
    return value === null ? [] : [[key, value]];
  }));
  return JSON.stringify({ format: "crossagents-preferences", version: 1, settings });
}

export function importPreferences(text: string, storage: Storage) {
  const file = JSON.parse(text);
  if (file?.format !== "crossagents-preferences" || file.version !== 1 || !file.settings
    || typeof file.settings !== "object" || Array.isArray(file.settings)) {
    throw new Error("无法识别 CrossAgents 个性化设置文件");
  }
  const incoming = KEYS.flatMap((key) => {
    if (!Object.hasOwn(file.settings, key)) return [];
    const value = file.settings[key];
    if (typeof value !== "string") throw new Error("设置文件里的值格式有误");
    JSON.parse(value);
    return [{ key, value, previous: storage.getItem(key) }];
  });
  let saved = 0;
  try {
    for (const { key, value } of incoming) {
      storage.setItem(key, value);
      saved++;
    }
  } catch (error) {
    for (const { key, previous } of incoming.slice(0, saved).reverse()) {
      if (previous === null) storage.removeItem(key);
      else storage.setItem(key, previous);
    }
    throw error;
  }
}
