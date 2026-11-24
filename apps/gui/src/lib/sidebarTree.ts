import type { Agent } from "@bindings/Agent";
import type { RunDetail } from "@bindings/RunDetail";
import { homeOf, isChat, lastWork, projectMembers, standaloneSessions, wsKey, wsName, type ProjectMember } from "./projectView.ts";

/** A sidebar row: the member, and when the work its summary names last moved. */
export type SidebarMember = ProjectMember & { lastAt: string | null };

export type SidebarGroup = {
  key: string;
  name: string;
  members: SidebarMember[];
};

/** Reorders `ids` by moving `fromId` before (or `after`) `toId`. Pure. */
export function reorderIds(ids: readonly string[], fromId: string, toId: string, after = false): string[] {
  if (fromId === toId || !ids.includes(fromId) || !ids.includes(toId)) return [...ids];
  const rest = ids.filter((id) => id !== fromId);
  const idx = rest.indexOf(toId);
  rest.splice(after ? idx + 1 : idx, 0, fromId);
  return rest;
}

/** Domain views own membership and stable ordering; this adds presentation preferences. */
export function sidebarGroups(
  agents: Agent[],
  runs: RunDetail[],
  projects: readonly { key: string; name: string }[],
  pins: ReadonlySet<string>,
  order: readonly string[] = [],
  sessionOrder: Readonly<Record<string, readonly string[]>> = {},
): SidebarGroup[] {
  const pinned = new Set([...pins].filter((k) => k.startsWith("s:")).map((k) => k.slice(2)));
  const members = projectMembers(agents, runs, pinned);
  const assigned = new Set([...members.values()].flat().map((m) => m.agent.id));
  const rank = (key: string) => (order.includes(key) ? order.indexOf(key) : order.length);
  // Newest first, with pins at top and per-group drag-and-drop order preserved; unranked new
  // sessions appear above ranked ones so they are never buried behind 更多.
  const sortAgents = (groupKey: string) => {
    const custom = sessionOrder[groupKey] ?? [];
    const customRank = (id: string) => (custom.length ? custom.indexOf(id) : -1);
    return (a: Agent, b: Agent) => {
      const pinDiff = Number(pinned.has(b.id)) - Number(pinned.has(a.id));
      if (pinDiff) return pinDiff;
      const ra = customRank(a.id);
      const rb = customRank(b.id);
      if (ra !== rb) {
        if (ra === -1) return -1;
        if (rb === -1) return 1;
        return ra - rb;
      }
      return b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id);
    };
  };
  const withWork = (m: ProjectMember, project: readonly RunDetail[]): SidebarMember => {
    const work = lastWork(m.agent.id, project);
    return { ...m, summary: work?.text ?? null, lastAt: work?.at || null };
  };
  const groups = projects
    .map((p) => {
      const project = runs.filter((d) => wsKey(d.run.repo) === p.key);
      const cmp = sortAgents(p.key);
      return {
        ...p,
        members: (members.get(p.key) ?? [])
          .filter((m) => !m.agent.archived_at)
          .sort((a, b) => cmp(a.agent, b.agent))
          .map((m) => withWork(m, project)),
      };
    })
    .sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key));
  const standalone = standaloneSessions(agents)
    .filter((a) => !assigned.has(a.id) && !a.archived_at)
    .sort(sortAgents("__chats"));
  if (standalone.length)
    groups.push({
      key: "__chats",
      name: "",
      members: standalone.map((agent) => ({
        agent,
        project: "",
        status: agent.status,
        summary: null,
        lastAt: null,
      })),
    });
  return groups;
}

export function searchGroups(groups: readonly SidebarGroup[], query: string): SidebarGroup[] {
  const q = query.trim().toLocaleLowerCase();
  if (!q) return [...groups];
  return groups
    .map((g) => ({
      ...g,
      members: g.members.filter((m) => [m.agent.name, m.agent.topic, m.summary].some((s) => s?.toLocaleLowerCase().includes(q))),
    }))
    .filter((g) => g.members.length);
}

/** Search reveals matches temporarily without modifying persisted folds. */
export const groupClosed = (collapsed: ReadonlySet<string>, key: string, searching: boolean) => !searching && collapsed.has(key);

export function capped<T>(items: T[], keep: (item: T) => boolean, cap: number, all: boolean): T[] {
  return all ? items : items.filter((item, i) => i < Math.max(0, cap) || keep(item));
}

/** How long since `iso`, in the few characters a row's end has: 刚刚, 40 分, 3 时, 2 天. */
export function shortSince(iso: string, now: number): string {
  const minutes = (now - Date.parse(iso)) / 60_000;
  if (!Number.isFinite(minutes)) return "";
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${Math.floor(minutes)} 分`;
  if (minutes < 60 * 24) return `${Math.floor(minutes / 60)} 时`;
  return `${Math.floor(minutes / (60 * 24))} 天`;
}

/** Filesystem paths and old role-only labels do not describe useful work. */
export function summaryText(value: string | null | undefined): string {
  const text = value?.trim() ?? "";
  return /^(?:[A-Za-z]:[\\/]|\/|\\\\|(?:主导|worker|reviewer|planner)$)/i.test(text) ? "" : text;
}

/** The one machine there is today: the centre, where every workspace lives. */
export const CENTRE_MACHINE = "centre";

/** A machine and the workspace groups on it. Nodes (B1) will add machines here. */
export type SidebarMachine = {
  key: string;
  groups: SidebarGroup[];
};

/** Groups workspace groups by machine (`machineKey` says which; the centre for now). Machines
 * with no group are kept for the centre, so its row is there before the first workspace. */
export function machineGroups(groups: readonly SidebarGroup[], machineKey: (g: SidebarGroup) => string = () => CENTRE_MACHINE): SidebarMachine[] {
  const by = new Map<string, SidebarGroup[]>([[CENTRE_MACHINE, []]]);
  for (const g of groups) {
    const key = machineKey(g);
    by.set(key, [...(by.get(key) ?? []), g]);
  }
  return [...by].map(([key, list]) => ({ key, groups: list }));
}

/** The fold key of a machine row; shares the groups' remembered set, so it cannot clash with a path. */
export const machineFoldKey = (machine: string) => `machine:${machine}`;

/** Which sidebar machine an agent belongs to: centre when unset or `"local"`, else its node id. */
export function agentMachine(agent: { node?: string | null }): string {
  const n = agent.node?.trim();
  return !n || n.toLowerCase() === "local" || n === CENTRE_MACHINE ? CENTRE_MACHINE : n;
}

/**
 * Builds the sidebar tree partitioned by machine: centre first, then remote nodes (in order),
 * then any historical node ids still referenced by agents. Each machine owns its own project
 * groups and its own standalone chats group so identical paths on different OSes never collide.
 */
export function buildMachineGroups(
  agents: Agent[],
  runs: RunDetail[],
  remoteNodeIds: readonly string[],
  pins: ReadonlySet<string>,
  order: readonly string[] = [],
  sessionOrder: Readonly<Record<string, readonly string[]>> = {},
): SidebarMachine[] {
  const keys: string[] = [CENTRE_MACHINE];
  const seen = new Set<string>(keys);
  for (const id of remoteNodeIds) {
    const trimmed = id.trim();
    if (!trimmed || trimmed.toLowerCase() === "local" || seen.has(trimmed)) continue;
    seen.add(trimmed);
    keys.push(trimmed);
  }
  for (const a of agents) {
    const k = agentMachine(a);
    if (!seen.has(k)) {
      seen.add(k);
      keys.push(k);
    }
  }

  return keys.map((key) => {
    const machineAgents = agents.filter((a) => agentMachine(a) === key);
    const machineRuns = key === CENTRE_MACHINE ? runs : [];
    const projMap = new Map<string, { key: string; name: string }>();
    for (const d of machineRuns) {
      const k = wsKey(d.run.repo);
      if (k && !projMap.has(k)) projMap.set(k, { key: k, name: wsName(d.run.repo) });
    }
    for (const a of machineAgents) {
      if (isChat(a)) continue;
      const h = homeOf(a);
      if (!h) continue;
      const k = wsKey(h);
      if (!projMap.has(k)) projMap.set(k, { key: k, name: wsName(h) });
    }
    const groups = sidebarGroups(machineAgents, machineRuns, [...projMap.values()], pins, order, sessionOrder);
    return { key, groups };
  });
}

/** Filters machine groups by search query, hiding machines with no matches while searching. */
export function searchMachineGroups(machines: readonly SidebarMachine[], query: string): SidebarMachine[] {
  if (!query.trim()) return [...machines];
  return machines
    .map((m) => ({ ...m, groups: searchGroups(m.groups, query) }))
    .filter((m) => m.groups.length > 0);
}
