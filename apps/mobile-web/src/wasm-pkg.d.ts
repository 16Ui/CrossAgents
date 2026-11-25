declare module "*crossagents_link_wasm.js" {
  export default function init(moduleOrPath?: unknown): Promise<unknown>;
  export function decode_invite(code: string): string;
  export function redeem_invite(code: string, deviceName: string, os: string): Promise<string>;
  export class WasmLinkSession {
    static connect(configJson: string): Promise<WasmLinkSession>;
    request(method: string, path: string, body?: string): Promise<string>;
    subscribe_events(
      since: number | undefined,
      onEvent: (json: string) => void,
      onError: (errJson: string) => void,
    ): Promise<{ cancel(): void }>;
    is_closed(): boolean;
    close(): Promise<void>;
  }
}
