import assert from "node:assert/strict";
import test from "node:test";
import { favoriteKey, toggleFavoriteList } from "./modelFavorites.ts";

test("favoriteKey encodes plugin, model, and optional account", () => {
  assert.equal(favoriteKey("codex", "gpt-5.4"), "codex:gpt-5.4");
  assert.equal(favoriteKey("codex", "gpt-5.4", "work"), "codex:gpt-5.4@work");
});

test("toggleFavoriteList adds to front and removes cleanly", () => {
  const a = toggleFavoriteList([], "codex:gpt-5.4");
  assert.deepEqual(a, ["codex:gpt-5.4"]);
  const b = toggleFavoriteList(a, "claude:opus");
  assert.deepEqual(b, ["claude:opus", "codex:gpt-5.4"]);
  const c = toggleFavoriteList(b, "codex:gpt-5.4");
  assert.deepEqual(c, ["claude:opus"]);
});
