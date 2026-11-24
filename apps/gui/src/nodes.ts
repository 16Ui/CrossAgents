// Execution nodes: the centre hub machine plus any connected (or known offline) remote nodes.
// Shared across Sidebar, Home composer, Session header, and Devices page.

import { useSyncExternalStore } from "react";
import { api } from "./api";
import { useHubData, usePlugins } from "./hub";
import { CENTRE_MACHINE } from "./lib/sidebarTree";

export type NodeOs = "windows" | "macos" | "linux" | undefined;

export function normalizeNodeOs(os?: string | null): NodeOs {
  const v = os?.trim().toLowerCase();
  if (v === "windows") return "windows";
  if (v === "macos" || v === "darwin") return "macos";
  if (v === "linux") return "linux";
  return undefined;
}

export function isRemoteNodeId(node?: string | null): node is string {
  const v = node?.trim();
  return !!v && v.toLowerCase() !== "local" && v !== CENTRE_MACHINE;
}

export function agentMachineKey(agent: { node?: string | null }): string {
  return isRemoteNodeId(agent.node) ? agent.node.trim() : CENTRE_MACHINE;
}

export function useNodes() {
  return useHubData(
    () => api.listNodes().then((r) => r.nodes),
    [],
    (e) => e.type === "node.changed" || e.type === "device.created" || e.type === "device.revoked",
  );
}

/**
 * Plugins & models for `nodeId` (empty / `"local"` / `CENTRE_MACHINE` uses the shared hub cache;
 * a remote node loads `GET /v1/plugins?node=<id>` and refreshes whenever `node.changed` arrives).
 */
export function useNodePlugins(nodeId?: string | null) {
  const remoteId = isRemoteNodeId(nodeId) ? nodeId.trim() : "";
  const local = usePlugins();
  const remote = useHubData(
    () => (remoteId ? api.listPlugins(remoteId).then((r) => r.plugins) : Promise.resolve([])),
    [remoteId],
    (e) => e.type === "node.changed" || e.type === "device.revoked",
    () => !!remoteId,
  );
  return remoteId ? remote : local;
}

// ---- Selected target node for new chats on Home ("" = centre machine) -------------------

const NODE_STORAGE = "crossagents.node";
const nodeListeners = new Set<() => void>();
let selectedNode = (() => {
  try {
    return localStorage.getItem(NODE_STORAGE) ?? "";
  } catch {
    return "";
  }
})();

export function selectNode(nodeId: string) {
  const next = isRemoteNodeId(nodeId) ? nodeId.trim() : "";
  if (selectedNode === next) return;
  selectedNode = next;
  try {
    localStorage.setItem(NODE_STORAGE, next);
  } catch {
    // Ignore storage failures.
  }
  nodeListeners.forEach((l) => l());
}

export function useSelectedNode(): string {
  return useSyncExternalStore(
    (l) => {
      nodeListeners.add(l);
      return () => nodeListeners.delete(l);
    },
    () => selectedNode,
  );
}
