// Browser regression: real React effects and the production hub.ts module, a fake bridge only.
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { api } from "../src/api";
import { startEvents, useHubData, usePlugins } from "../src/hub";
import { useQuota } from "../src/views/Quota";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
const delay = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>((r) => { resolve = r; }); return { promise, resolve }; };
const assert = (ok: unknown, message: string) => { if (!ok) throw new Error(message); };
let status!: (value: string) => void;
let identity!: (value: unknown) => void;
let event!: (value: string) => void;
let name = "A", reads = 0, dataReads = 0;
let quotaReads = 0;
let pending: ReturnType<typeof deferred<{ plugins: never[] }>> | undefined;
let infoPending: ReturnType<typeof deferred<{ api: string }>> | undefined;
let oldData: ReturnType<typeof deferred<string>> | undefined;
const connection = (id: string) => ({ mode: "remote", remote: { deviceId: id, pairedAt: id, target: { host: id } } });
const initialIdentity = deferred<unknown>();
Object.assign(window, { crossagents: {
  connection: { onState: (fn: typeof identity) => { identity = fn; }, get: () => initialIdentity.promise },
  onStatus: (fn: typeof status) => { status = fn; }, onEvent: (fn: typeof event) => { event = fn; }, startEvents() {},
  request() { throw new Error("unexpected real API request"); },
} });
Object.assign(api, {
  info: () => infoPending?.promise ?? Promise.resolve({ api: "v2" }),
  listPlugins: () => { reads++; return pending?.promise ?? Promise.resolve({ plugins: [name] }); },
  quota: () => { quotaReads++; return Promise.resolve({ accounts: [], providers: [] }); },
});
let catalogs: ReturnType<typeof usePlugins>[] = [];
let resource: ReturnType<typeof useHubData<string>>;
const load = () => { dataReads++; return oldData?.promise ?? Promise.resolve(name); };
function Probe({ index }: { index: number }) {
  catalogs[index] = usePlugins();
  resource = useHubData(load, [], () => true);
  return <span>{JSON.stringify(catalogs[index].data)}</span>;
}
let quota: ReturnType<typeof useQuota>;
function QuotaProbe() {
  quota = useQuota();
  return null;
}
let root = createRoot(document.getElementById("root")!);
const tick = async (ms = 0) => { await act(async () => { await delay(ms); }); };
const emit = async (type: string) => { await act(async () => event(JSON.stringify({ type }))); };
const checkCatalog = (expected: string) => assert(JSON.stringify(catalogs[0].data) === JSON.stringify([expected]), `catalog expected ${expected}: ${JSON.stringify(catalogs[0])}`);
const checks: string[] = [];
async function run() {
  startEvents();
  identity(connection("A"));
  await act(async () => { root.render(<><Probe index={0} /><Probe index={1} /></>); });
  assert(reads === 0 && dataReads === 0, "no resource requests before version validation");
  await act(async () => status("connected")); await tick(300);
  checkCatalog("A"); assert(reads === 1, `multi subscriber cold read coalesced: ${reads}`);
  assert(!resource.loading && resource.data === "A", "initial mounted resource loaded");
  checks.push("cold mount / version gate / multiple subscribers");
  await act(async () => root.unmount());
  root = createRoot(document.getElementById("root")!);
  await act(async () => root.render(<><Probe index={0} /><Probe index={1} /></>));
  checkCatalog("A"); assert(reads === 1, "return mount reuses usable list without new read");
  checks.push("unmount / remount preserves list");
  // A response for the old connection must never enter either real hook.
  pending = deferred(); oldData = deferred();
  await act(async () => { void catalogs[0].reload(); resource.reload(); });
  const oldCatalog = pending, previousData = oldData;
  pending = undefined; oldData = undefined; name = "B";
  infoPending = deferred();
  await act(async () => identity(connection("B")));
  assert(catalogs[0].data === undefined && resource.data === undefined && resource.loading, "identity clears mounted data while transport stays connected");
  await act(async () => { oldCatalog.resolve({ plugins: ["STALE"] as never[] }); previousData.resolve("STALE"); });
  assert(catalogs[0].data === undefined && resource.data === undefined, "late cross-identity response rejected during version check");
  await act(async () => infoPending!.resolve({ api: "v2" })); infoPending = undefined; await tick(300);
  checkCatalog("B"); assert(!resource.loading && resource.data === "B", "mounted resource recovers after identity/version check");
  await act(async () => initialIdentity.resolve(connection("A")));
  checkCatalog("B");
  checks.push("connected identity switch / old responses rejected / mounted loading recovers");
  name = "C"; await act(async () => status("offline: test"));
  assert(catalogs[0].data === undefined && resource.data === undefined, "disconnect clears both hooks");
  const offlineReads = reads; await emit("plugin.changed"); await tick(300);
  assert(reads === offlineReads, "offline events cannot query unvalidated hub");
  await act(async () => status("connected")); await tick(300);
  checkCatalog("C"); assert(!resource.loading && resource.data === "C", "reconnect restores mounted resource");
  checks.push("disconnect / offline events / reconnect");
  for (const type of ["plugin.changed", "account.created", "account.removed", "account.login_finished", "account.quota_updated", "cli.install.finished"]) {
    name = type; const before = reads;
    await emit(type); await emit(type); await tick(300);
    checkCatalog(type); assert(reads === before + 1, `${type} event burst merged`);
  }
  checks.push("visibility/plugin/account/CLI event refresh bursts");
  const before = reads;
  await act(async () => { window.dispatchEvent(new Event("focus")); window.dispatchEvent(new Event("focus")); });
  await tick(300); assert(reads === before + 1, "focus refresh coalesces");
  // A version recheck can clear a mounted view while an earlier read is still in flight.
  oldData = deferred(); await act(async () => resource.reload()); const duringCheck = oldData;
  oldData = undefined; infoPending = deferred();
  await act(async () => status("connected"));
  await act(async () => duringCheck.resolve("STALE"));
  assert(resource.loading && resource.data === undefined, "version check rejects previous in-flight resource");
  name = "checked"; await act(async () => infoPending!.resolve({ api: "v2" })); infoPending = undefined;
  await tick(300); assert(!resource.loading && resource.data === "checked", "same-transport version check cannot leave mounted resource loading");
  checks.push("focus coalescing / same-transport version check and loading recovery");
  infoPending = deferred(); await act(async () => status("connected"));
  await act(async () => infoPending!.resolve({ api: "incompatible" })); infoPending = undefined;
  const refusedReads = reads; await emit("plugin.changed"); await tick(300);
  assert(resource.loading && resource.data === undefined && reads === refusedReads, "mismatched version stays isolated");
  name = "compatible"; await act(async () => identity(connection("compatible"))); await tick(300);
  checkCatalog("compatible"); assert(!resource.loading && resource.data === "compatible", "switching away from incompatible hub recovers mounted hooks");
  checks.push("version mismatch / compatible identity switch recovery / delayed identity.get ignored");
  await act(async () => root.unmount());
  const now = Date.now; Date.now = () => now() + 31_000;
  const staleReads = reads;
  pending = deferred();
  root = createRoot(document.getElementById("root")!);
  await act(async () => root.render(<><Probe index={0} /><Probe index={1} /></>));
  Date.now = now;
  checkCatalog("compatible"); assert(reads === staleReads + 1, "expired remount revalidates once for multiple subscribers");
  assert(!catalogs[0].loading, "usable stale list stays visible during revalidation");
  await act(async () => pending!.resolve({ plugins: ["revalidated"] as never[] })); pending = undefined;
  checkCatalog("revalidated");
  checks.push("expired remount revalidates / usable list retained");
  await act(async () => root.unmount());

  let focused = false;
  let visibility = "visible";
  Object.defineProperty(document, "hasFocus", { configurable: true, value: () => focused });
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  root = createRoot(document.getElementById("root")!);
  await act(async () => root.render(<QuotaProbe />));
  await emit("account.quota_updated"); await tick(300);
  assert(quotaReads === 0, "an unfocused window skips quota on mount and account events");
  focused = true;
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  assert(quotaReads === 1 && quota.data, "focusing the window refreshes quota");
  visibility = "hidden";
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); quota.reload(); });
  await emit("account.quota_updated"); await tick(300);
  assert(quotaReads === 1 && quota.data, "a hidden window keeps its quota data and skips reloads");
  visibility = "visible";
  await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
  assert(quotaReads === 2, "restoring the visible window refreshes quota");
  await act(async () => root.unmount());
  checks.push("background quota requests paused / foreground quota refresh resumes");
  return { checks, reads, dataReads };
}
Object.assign(window, { regression: run().then((result) => ({ ok: true, ...result }), (error) => ({ ok: false, error: String(error), checks })) });
