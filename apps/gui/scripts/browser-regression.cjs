const { app, BrowserWindow } = require("electron");
app.disableHardwareAcceleration();
app.commandLine.appendSwitch("no-proxy-server");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
setTimeout(() => { console.error("regression timed out"); app.exit(2); }, 60_000).unref();
app.whenReady().then(async () => {
  try {
    const win = new BrowserWindow({ show: false, width: 1280, height: 800, useContentSize: true, webPreferences: { offscreen: true } });
    win.webContents.on("console-message", (_event, ...args) => { if (args[0] === 3) console.error(...args); });
    if (process.argv.includes("--ui")) {
      const result = await require("./ui-regression.cjs")(win, process.env.CA_REGRESSION_URL);
      const out = process.argv.find((arg) => arg.startsWith('--out='))?.slice(6);
      if (out) require('node:fs').writeFileSync(out, JSON.stringify({electron:process.versions.electron,chromium:process.versions.chrome,viewport:[1280,800],...result},null,2)+'\n');
      return app.exit(0);
    }
    await win.loadURL(`${process.env.CA_REGRESSION_URL}/hub-regression`);
    for (let i = 0; i < 100 && !await win.webContents.executeJavaScript("!!window.regression"); i++) await sleep(50);
    const result = await win.webContents.executeJavaScript("window.regression");
    console.log(JSON.stringify(result, null, 2));
    app.exit(result?.ok ? 0 : 1);
  } catch (error) { console.error(error); app.exit(1); }
});
