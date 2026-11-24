import assert from "node:assert/strict";
import test from "node:test";
import { reorderIds } from "./sidebarTree.ts";

test("reorderIds moves item before target", () => {
  assert.deepEqual(reorderIds(["a", "b", "c", "d"], "d", "b", false), ["a", "d", "b", "c"]);
});

test("reorderIds moves item after target", () => {
  assert.deepEqual(reorderIds(["a", "b", "c", "d"], "a", "c", true), ["b", "c", "a", "d"]);
});

test("reorderIds is a no-op for unknown or identical ids", () => {
  assert.deepEqual(reorderIds(["a", "b"], "a", "a", false), ["a", "b"]);
  assert.deepEqual(reorderIds(["a", "b"], "x", "b", false), ["a", "b"]);
});
