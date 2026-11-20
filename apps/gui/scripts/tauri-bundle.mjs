import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const gui = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(gui, "../..");
const windows = process.platform === "win32";
if (!windows && process.platform !== "darwin") throw new Error("Tauri desktop bundles are built on Windows or macOS");

const host = execFileSync("rustc", ["-vV"], { cwd: repo, encoding: "utf8" })
  .split(/\r?\n/).find((line) => line.startsWith("host: "))?.slice(6);
if (!host) throw new Error("Could not determine the Rust host target");
const version = JSON.parse(readFileSync(join(gui, "package.json"), "utf8")).version;
const run = (program, args, cwd = gui) => execFileSync(program, args, { cwd, stdio: "inherit" });
const npm = (args) => run(process.execPath, [process.env.npm_execpath, ...args]);

run("cargo", ["build", "--release", "--locked", "-p", "crossagents-cli", "-p", "crossagents-link"], repo);
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
  cwd: repo, encoding: "utf8",
}));
const suffix = windows ? ".exe" : "";
const binaries = join(gui, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
for (const name of ["crossagents", "crossagents-link"]) {
  const source = join(metadata.target_directory, "release", name + suffix);
  const output = execFileSync(source, ["--version"], { encoding: "utf8" }).trim();
  if (output.split(/\s+/).at(-1) !== version) throw new Error(`${name} version does not match GUI ${version}: ${output}`);
  copyFileSync(source, join(binaries, `${name}-${host}${suffix}`));
}

npm(["exec", "--", "tsc", "--noEmit"]);
npm(["exec", "--", "vite", "build", "--mode", "tauri", "--outDir", "dist-tauri"]);
const configs = ["--config", "src-tauri/tauri.bundle.conf.json"];
const updateEndpoint = process.env.CROSSAGENTS_TAURI_UPDATE_ENDPOINT;
const updatePubkey = process.env.CROSSAGENTS_TAURI_UPDATE_PUBKEY;
if (updateEndpoint || updatePubkey) {
  const endpoint = updateEndpoint && new URL(updateEndpoint);
  if (!endpoint || endpoint.protocol !== "https:" || !updatePubkey || !process.env.TAURI_SIGNING_PRIVATE_KEY) {
    throw new Error("Signed builds require an HTTPS update endpoint, public key and Tauri signing private key");
  }
  configs.push("--config", JSON.stringify({
    bundle: { createUpdaterArtifacts: true },
    plugins: { updater: { endpoints: [endpoint.href], pubkey: updatePubkey.trim(), requireSignedVersion: true } },
  }));
}
if (!windows) {
  npm(["exec", "--", "tauri", "icon", "build/icon.png", "--output", "src-tauri/binaries/icons"]);
  configs.push("--config", JSON.stringify({ bundle: { icon: ["binaries/icons/icon.icns", "../build/icon.png"] } }));
}
npm(["exec", "--", "tauri", "build", ...configs,
  "--features", "bundled-hub", "--bundles", windows ? "nsis" : "app,dmg", "--", "--locked"]);
