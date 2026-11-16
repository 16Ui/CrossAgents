import assert from "node:assert/strict";
import { test } from "node:test";
import { ResourceCache } from "./resourceCache.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

test("cold mounts share one read and returning pages keep the available list", async () => {
  const read = deferred<string[]>();
  let calls = 0;
  const cache = new ResourceCache(() => { calls++; return read.promise; });
  cache.ensure(); cache.ensure(); cache.ensure();
  await Promise.resolve();
  assert.equal(calls, 1);
  read.resolve(["fast-model"]);
  await new Promise((done) => setImmediate(done));
  cache.ensure();
  assert.equal(calls, 1);
  assert.deepEqual(cache.snapshot().data, ["fast-model"]);
});

test("refresh bursts make one follow-up and never blank a usable list", async () => {
  const reads = [deferred<string[]>(), deferred<string[]>()];
  let calls = 0;
  const cache = new ResourceCache(() => reads[calls++].promise);
  const first = cache.refresh();
  for (let i = 0; i < 20; i++) void cache.refresh();
  reads[0].resolve(["old"]);
  await first;
  await Promise.resolve();
  assert.equal(calls, 2);
  assert.deepEqual(cache.snapshot().data, ["old"]);
  reads[1].resolve(["visible-after-change"]);
  await new Promise((done) => setImmediate(done));
  assert.deepEqual(cache.snapshot().data, ["visible-after-change"]);
});

test("hub switches and reconnects discard both old data and late responses", async () => {
  const reads = [deferred<string[]>(), deferred<string[]>()];
  let calls = 0;
  const cache = new ResourceCache(() => reads[calls++].promise);
  const old = cache.refresh();
  await Promise.resolve();
  cache.reset();
  const fresh = cache.refresh();
  reads[0].resolve(["hub-A-private"]);
  await old;
  assert.equal(cache.snapshot().data, undefined);
  reads[1].resolve(["hub-B"]);
  await fresh;
  assert.deepEqual(cache.snapshot().data, ["hub-B"]);
});

test("failed refresh preserves available models and can recover", async () => {
  let fail = false;
  const cache = new ResourceCache(async () => {
    if (fail) throw new Error("offline");
    return ["available"];
  });
  await cache.refresh();
  fail = true;
  await cache.refresh();
  assert.deepEqual(cache.snapshot().data, ["available"]);
  assert.match(cache.snapshot().error!, /offline/);
  fail = false;
  await cache.refresh();
  assert.equal(cache.snapshot().error, undefined);
});

test("returning to an expired catalog revalidates while retaining usable models", async () => {
  let reads = 0;
  const fresh = deferred<string[]>();
  const cache = new ResourceCache(() => ++reads === 1 ? Promise.resolve(["kept"]) : fresh.promise, 0);
  await cache.refresh();
  cache.ensure(); cache.ensure();
  await Promise.resolve();
  assert.equal(reads, 2);
  assert.deepEqual(cache.snapshot().data, ["kept"]);
  fresh.resolve(["updated"]);
  await new Promise((done) => setImmediate(done));
  assert.deepEqual(cache.snapshot().data, ["updated"]);
});
