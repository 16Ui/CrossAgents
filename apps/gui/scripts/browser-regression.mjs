// Uses the repository's existing Electron and Vite; no user hub, credentials or model calls.
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { createServer } from "vite";
import { execFileSync } from "node:child_process";
const root = fileURLToPath(new URL("../", import.meta.url));
// Requiring electron can auto-download a missing binary. Tests must never install software.
const electronPackage = dirname(createRequire(import.meta.url).resolve("electron/package.json"));
const electronPathFile = join(electronPackage, "path.txt");
if (!existsSync(electronPathFile)) throw new Error("Electron binary is not installed; this test does not download it");
const electron = join(electronPackage, "dist", readFileSync(electronPathFile, "utf8").trim());
if (!existsSync(electron)) throw new Error("Electron binary is missing; this test does not download it");
const ui = process.argv.includes("--ui");
const baseline = process.argv.find((a) => a.startsWith("--baseline="))?.split("=")[1];
const baselineFiles = ["src/hub.ts", "src/views/Home.tsx", "src/modelPick.tsx", "src/views/Agents.tsx", "src/views/Run.tsx", "src/views/SessionComposer.tsx", "src/views/Settings.tsx", "src/styles.css"];
const sources = new Map(baseline ? baselineFiles.map((file) => [join(root, file).replaceAll("\\", "/"), execFileSync("git", ["show", `${baseline}:apps/gui/${file}`], { cwd: root, encoding: "utf8" })]) : []);
const server = await createServer({ root, configFile: join(root, "vite.config.ts"), mode: "mock", logLevel: "error", cacheDir: join(root, "node_modules/.vite-regression"), server: { host: "127.0.0.1", port: 0, strictPort: false }, plugins: [{
  name: "regression-controlled-sources", enforce: "pre",
  load(id) { return sources.get(id.split("?")[0].replaceAll("\\", "/")); },
  transform(code, id) {
    if (!ui) return;
    if (id.endsWith("/src/dev/MockBridge.ts")) return code.replace("for (const step of this.script)", "for (const step of this.script.slice(0, 0))");
    if (id.endsWith("/src/main.tsx")) return code.replace("if (start) {", `if (start) {
      localStorage.setItem('crossagents.mode', JSON.stringify('run'));
      const original = window.crossagents.request.bind(window.crossagents);
      window.__catalogReads = 0;
      window.crossagents.request = async (...args) => {
        if (args[1] === '/v1/plugins') { window.__catalogReads++; await new Promise(r => setTimeout(r, 800)); }
        return original(...args);
      };
    `);
  },
}, {
  name: "hub-regression-page", configureServer(server) {
    server.middlewares.use("/hub-regression", async (_req, res) => {
      res.setHeader("Content-Type", "text/html");
      res.end(await server.transformIndexHtml("/hub-regression", '<div id="root"></div><script type="module" src="/scripts/hub-integration.tsx"></script>'));
    });
  },
}] });
await server.listen();
const env = { ...process.env, CA_REGRESSION_URL: `http://127.0.0.1:${server.httpServer.address().port}` };
delete env.ELECTRON_RUN_AS_NODE;
try {
  const code = await new Promise((resolve, reject) => {
    const child = spawn(electron, [join(root, "scripts/browser-regression.cjs"), ...process.argv.slice(2)], { env, stdio: "inherit" });
    child.on("error", reject); child.on("exit", resolve);
  });
  process.exitCode = code ?? 1;
} finally { await server.close(); }
