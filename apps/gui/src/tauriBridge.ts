import type { CrossAgentsBridge, HubResponse } from "./bridge";

type StreamFrame =
  | { type: "connected" }
  | { type: "event"; data: string; id: string };

type TauriCore = {
  invoke<T>(command: string, args: Record<string, unknown>): Promise<T>;
  Channel: new <T>(onmessage: (message: T) => void) => object;
};

/** Installs only the Hub transport. Desktop features without a Tauri implementation stay absent. */
export function installTauriBridge() {
  const tauri = (window as Window & { __TAURI__?: { core?: TauriCore } }).__TAURI__?.core;
  if (!tauri || window.crossagents) return;
  const { Channel, invoke } = tauri;

  const events = new Set<(data: string) => void>();
  const statuses = new Set<(status: string) => void>();
  let started = false;
  let lastId: string | null = null;
  const setStatus = (status: string) => statuses.forEach((listener) => listener(status));

  async function relay() {
    for (;;) {
      setStatus("connecting");
      const frames = new Channel<StreamFrame>((frame) => {
        if (frame.type === "connected") {
          setStatus("connected");
        } else {
          if (frame.id) lastId = frame.id;
          events.forEach((listener) => listener(frame.data));
        }
      });
      try {
        await invoke("hub_events", { lastId, frames });
        setStatus("offline: event stream ended");
      } catch (error) {
        setStatus(`offline: ${String(error)}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
  }

  const bridge: CrossAgentsBridge = {
    request: (method, path, body) => invoke<HubResponse>("hub_request", { method, path, body }),
    startEvents() {
      if (started) return;
      started = true;
      void relay();
    },
    onEvent(listener) {
      events.add(listener);
      return () => events.delete(listener);
    },
    onStatus(listener) {
      statuses.add(listener);
      return () => statuses.delete(listener);
    },
  };
  window.crossagents = bridge;
}
