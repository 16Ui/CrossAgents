import assert from "node:assert/strict";
import { test } from "node:test";
import { exportPreferences, importPreferences } from "./preferencesTransfer.ts";

const storage = (entries: Array<[string, string]> = []) => {
  const values = new Map(entries);
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } as unknown as Storage;
};

test("transfer preserves selected preferences without copying other local storage", () => {
  const theme = '{"mode":"dark","paper":"ink"}';
  const from = storage([["crossagents.theme", theme], ["token", '"private"']]);
  const to = storage();
  const exported = JSON.parse(exportPreferences(from));
  assert.deepEqual(exported.settings, { "crossagents.theme": theme });
  exported.settings.token = '"private"';
  importPreferences(JSON.stringify(exported), to);
  assert.equal(to.getItem("crossagents.theme"), theme);
  assert.equal(to.getItem("token"), null);
});

test("a malformed entry leaves existing preferences in place", () => {
  const to = storage([["crossagents.theme", '"old"']]);
  const file = JSON.stringify({
    format: "crossagents-preferences",
    version: 1,
    settings: { "crossagents.theme": '"new"', "crossagents.chatProfile": "invalid" },
  });
  assert.throws(() => importPreferences(file, to));
  assert.equal(to.getItem("crossagents.theme"), '"old"');
});
