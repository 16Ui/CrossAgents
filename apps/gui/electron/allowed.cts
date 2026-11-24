// Which hub endpoints the page may reach through the main process (docs/API-AUTH.md §2, §4).
// Kept apart from main.cts so a test can check it against src/api.ts without Electron.

export type Level = "read" | "write" | "admin";
type Route = { method: string; path: string; level: Level };

/**
 * What the page may ask the hub for. It mirrors `src/api.ts`: endpoints the GUI does not
 * call are refused here, so a page that got script injected cannot reach `/v1/shutdown`,
 * delete an account it never shows, or touch the agent-token routes. Adding a call to
 * `src/api.ts` means adding a row to `shared/hub-routes.json`; `allowed.test.cts` fails
 * until it is done.
 *
 * `level` is not enforced here: the hub checks the token's level itself (crates/hub/src/access.rs).
 * It is kept equal to the hub's table so the two can be compared at a glance.
 */
const routes = require("../shared/hub-routes.json") as Route[];
export const ALLOWED = routes.map(({ method, path, level }) => ({ method, path: new RegExp(path), level }));

/** How long the main process waits for an answer before giving up on the hub. */
export const REQUEST_TIMEOUT_MS = 30_000;
/** Compacting answers when the summary is written: minutes for a long history. */
export const COMPACT_TIMEOUT_MS = 15 * 60_000;

export function timeoutFor(method: string, path: string): number {
  return method === "POST" && /^\/v1\/agents\/[^/]+\/compact$/.test(route(path)) ? COMPACT_TIMEOUT_MS : REQUEST_TIMEOUT_MS;
}

/** The path without its query string: `/v1/notes?scope=run` matches `/v1/notes`. */
export function route(path: string): string {
  const i = path.indexOf("?");
  return i < 0 ? path : path.slice(0, i);
}

export function allowed(method: string, path: string): boolean {
  const only = route(path);
  return ALLOWED.some((r) => r.method === method && r.path.test(only));
}
