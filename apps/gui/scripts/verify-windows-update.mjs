import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createReadStream, existsSync, readFileSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { load } from "js-yaml";

const gui = new URL("../", import.meta.url);
const output = process.argv[2] ?? fileURLToPath(new URL("release/", gui));
const { version, build } = JSON.parse(readFileSync(new URL("package.json", gui), "utf8"));
const feed = load(readFileSync(join(output, "win-unpacked", "resources", "app-update.yml"), "utf8"));
const { provider, owner, repo } = build.publish[0];
assert.equal(feed.provider, provider);
assert.equal(feed.owner, owner);
assert.equal(feed.repo, repo);
assert.equal(`${owner}/${repo}`.toLowerCase(), (process.env.GITHUB_REPOSITORY ?? "16Ui/CrossAgents").toLowerCase(),
  "the installer must check the repository that publishes it");

const manifest = load(readFileSync(join(output, "latest.yml"), "utf8"));
assert.equal(manifest.version, version, "update version must match the packaged application");
assert.equal(manifest.files?.length, 1, "expected one x64 installer in latest.yml");
const [installer] = manifest.files;
const name = decodeURIComponent(installer.url);
assert.equal(name, basename(name), "update metadata must point to a release asset filename");
assert.match(name, /\.exe$/i);
assert.equal(manifest.path, installer.url);
assert.equal(manifest.sha512, installer.sha512);
const file = join(output, name);
assert.equal(statSync(file).size, installer.size);
const hash = createHash("sha512");
for await (const bytes of createReadStream(file)) hash.update(bytes);
assert.equal(hash.digest("base64"), installer.sha512, "installer does not match the update checksum");
assert.ok(existsSync(file + ".blockmap"), "missing the differential update blockmap");
console.log(`Windows update ${version}: ${owner}/${repo}, installer and checksum verified`);
