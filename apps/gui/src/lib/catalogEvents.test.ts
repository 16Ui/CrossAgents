import assert from "node:assert/strict";
import { test } from "node:test";
import { catalogEvent } from "./catalogEvents.ts";

test("account, model visibility and CLI installation events refresh all model pickers", () => {
  for (const type of ["account.created", "account.removed", "account.login_finished", "plugin.changed"]) {
    assert.equal(catalogEvent(type), "reset", type);
  }
  for (const type of ["account.quota_updated", "cli.install.started", "cli.install.finished"]) {
    assert.equal(catalogEvent(type), "refresh", type);
  }
  assert.equal(catalogEvent("agent.message"), undefined);
});
