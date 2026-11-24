import { basename } from "node:path";
import { readFileSync, statSync } from "node:fs";

const [base, ...inputs] = process.argv.slice(2);
if (!base || !inputs.length) {
  throw new Error("Usage: node scripts/tauri-update-manifest.mjs https://host/path/ platform=installer [...]");
}
const version = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;
const root = new URL(base);
if (root.protocol !== "https:" || root.username || root.password || root.search || root.hash || !root.pathname.endsWith("/")) {
  throw new Error("The release URL must be an HTTPS directory ending in /");
}

const platforms = {};
for (const input of inputs) {
  const separator = input.indexOf("=");
  if (separator < 1) throw new Error(`Missing platform for artifact: ${input}`);
  const platform = input.slice(0, separator);
  const artifact = input.slice(separator + 1);
  const extension = platform === "windows-x86_64" ? ".exe" : platform === "darwin-aarch64" ? ".app.tar.gz" : undefined;
  if (!extension || platforms[platform] || !artifact.endsWith(extension) || !statSync(artifact).isFile()) {
    throw new Error(`Unexpected or duplicate artifact: ${input}`);
  }
  const signature = readFileSync(artifact + ".sig", "utf8").trim();
  if (!signature || signature.length > 32768 || !/^[A-Za-z0-9+/=]+$/.test(signature)) {
    throw new Error(`Invalid Tauri signature for ${basename(artifact)}`);
  }
  const signatureComment = Buffer.from(signature, "base64").toString("utf8").split(/\r?\n/)[2];
  if (!signatureComment?.startsWith("trusted comment: ") || !signatureComment.split("\t").includes(`version:${version}`)) {
    throw new Error(`Tauri signature must bind version ${version} for ${basename(artifact)}`);
  }
  platforms[platform] = {
    url: new URL(encodeURIComponent(basename(artifact)), root).href,
    signature,
  };
}

process.stdout.write(JSON.stringify({ version, platforms }, null, 2) + "\n");
