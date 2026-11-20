(() => {
  const entries = [];
  Object.defineProperty(window, "__crossagentsConsole", { value: entries, configurable: false });
  for (const level of ["debug", "info", "log", "warn", "error"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      const text = args.map((arg) => {
        try {
          return typeof arg === "string" ? arg : JSON.stringify(arg) ?? String(arg);
        } catch {
          return String(arg);
        }
      }).join(" ").slice(0, 1000);
      entries.push({ level, text, ts: Date.now() });
      if (entries.length > 150) entries.shift();
      original(...args);
    };
  }
})();
