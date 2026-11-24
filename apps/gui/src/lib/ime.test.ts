import assert from "node:assert/strict";
import test from "node:test";
import { IME_END_GRACE_MS, isImeKeyDown } from "./ime.ts";

test("detects active composition via isComposing, composing flag, or keyCode 229", () => {
  assert.equal(isImeKeyDown({ isComposing: true, key: "Enter", composing: false, msSinceCompositionEnd: 999 }), true);
  assert.equal(isImeKeyDown({ isComposing: false, key: "Enter", composing: true, msSinceCompositionEnd: 999 }), true);
  assert.equal(isImeKeyDown({ isComposing: false, keyCode: 229, key: "Enter", composing: false, msSinceCompositionEnd: 999 }), true);
});

test("blocks trailing Enter right after compositionend (macOS WebKit quirk)", () => {
  assert.equal(isImeKeyDown({ isComposing: false, key: "Enter", composing: false, msSinceCompositionEnd: 12 }), true);
  assert.equal(isImeKeyDown({ isComposing: false, key: "Enter", composing: false, msSinceCompositionEnd: IME_END_GRACE_MS }), false);
  assert.equal(isImeKeyDown({ isComposing: false, key: "a", composing: false, msSinceCompositionEnd: 12 }), false);
});
