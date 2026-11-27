export type ChatProfile = {
  name: string;
  /** A square image data URL (or legacy raw base64), synced to the connected hub and cached locally. */
  avatar: string | null;
  showAvatars: boolean;
};

export type StoredChatProfile = ChatProfile & { synced?: boolean };

/** Pure reconciliation rule between local cached profile and hub profile. */
export function reconcileChatProfile(
  local: StoredChatProfile,
  remote: { name?: string | null; avatar?: string | null } | null | undefined,
): { next: StoredChatProfile; push?: { name: string; avatar: string | null } } {
  const remoteName = typeof remote?.name === "string" ? remote.name.slice(0, 32) : "";
  const remoteAvatar = typeof remote?.avatar === "string" && remote.avatar ? remote.avatar : null;
  if (remoteName || remoteAvatar) {
    // Never let an earlier hub profile without an avatar wipe out an unsynced local avatar
    // (for instance when another device connected first with only a name).
    if (!remoteAvatar && local.avatar && !local.synced) {
      const mergedName = remoteName || local.name;
      return {
        next: { ...local, name: mergedName, avatar: local.avatar, synced: false },
        push: { name: mergedName, avatar: local.avatar },
      };
    }
    return {
      next: { ...local, name: remoteName, avatar: remoteAvatar, synced: true },
    };
  }
  if (local.name || local.avatar) {
    return {
      next: local,
      push: { name: local.name, avatar: local.avatar },
    };
  }
  return { next: { ...local, synced: true } };
}
