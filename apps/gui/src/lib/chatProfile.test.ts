import assert from "node:assert/strict";
import test from "node:test";
import { reconcileChatProfile } from "./chatProfileSync.ts";

test("preserves and pushes an unsynced local avatar when remote only has a name", () => {
  const res = reconcileChatProfile(
    { name: "Link", avatar: "data:image/webp;base64,AAA", showAvatars: true, synced: false },
    { name: "Link", avatar: null },
  );
  assert.equal(res.next.avatar, "data:image/webp;base64,AAA");
  assert.deepEqual(res.push, { name: "Link", avatar: "data:image/webp;base64,AAA" });
});

test("clears local avatar when already synced and another device reset it on the hub", () => {
  const res = reconcileChatProfile(
    { name: "Link", avatar: "data:image/webp;base64,AAA", showAvatars: true, synced: true },
    { name: "Link", avatar: null },
  );
  assert.equal(res.next.avatar, null);
  assert.equal(res.next.synced, true);
  assert.equal(res.push, undefined);
});

test("adopts hub avatar on a fresh remote device", () => {
  const res = reconcileChatProfile(
    { name: "", avatar: null, showAvatars: true, synced: false },
    { name: "Link", avatar: "data:image/png;base64,BBB" },
  );
  assert.equal(res.next.name, "Link");
  assert.equal(res.next.avatar, "data:image/png;base64,BBB");
  assert.equal(res.next.synced, true);
  assert.equal(res.push, undefined);
});

test("pushes local profile up when hub has no saved profile yet", () => {
  const res = reconcileChatProfile(
    { name: "Link", avatar: "data:image/png;base64,CCC", showAvatars: true, synced: false },
    { name: "", avatar: null },
  );
  assert.deepEqual(res.push, { name: "Link", avatar: "data:image/png;base64,CCC" });
});
