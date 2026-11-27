import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const CHUNK_BYTES = 256 * 1024; // 256 KiB raw (~110 KiB gzipped on GitHub Pages)
const assetsDir = resolve("dist/assets");
const files = readdirSync(assetsDir);
const wasmFile = files.find((f) => /^crossagents_link_wasm_bg-[A-Za-z0-9_-]+\.wasm$/.test(f));

if (!wasmFile) {
  console.error("split-wasm: no crossagents_link_wasm_bg-*.wasm found in dist/assets");
  process.exit(1);
}

const fullPath = join(assetsDir, wasmFile);
const data = readFileSync(fullPath);
const stem = wasmFile.slice(0, -".wasm".length);
const chunks = [];

for (let offset = 0, idx = 0; offset < data.byteLength; offset += CHUNK_BYTES, idx++) {
  const slice = data.subarray(offset, Math.min(offset + CHUNK_BYTES, data.byteLength));
  const partName = `${stem}.part-${String(idx).padStart(2, "0")}.wasm`;
  writeFileSync(join(assetsDir, partName), slice);
  chunks.push({ file: partName, bytes: slice.byteLength });
}

const manifest = {
  file: wasmFile,
  totalBytes: data.byteLength,
  chunkBytes: CHUNK_BYTES,
  chunks,
};

const manifestName = `${wasmFile}.json`;
writeFileSync(join(assetsDir, manifestName), JSON.stringify(manifest));
console.log(
  `split-wasm: ${wasmFile} (${data.byteLength} bytes) -> ${chunks.length} chunks (${CHUNK_BYTES}B each) + ${manifestName}`,
);
