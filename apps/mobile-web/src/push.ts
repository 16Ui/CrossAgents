import type { HubTransport } from "./transport";

export function isIOS(): boolean {
  const ua = navigator.userAgent || "";
  return /iPhone|iPad|iPod/i.test(ua) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
}

export function isStandalonePwa(): boolean {
  const mq = window.matchMedia?.("(display-mode: standalone)")?.matches ?? false;
  const nav = (navigator as unknown as { standalone?: boolean }).standalone === true;
  return mq || nav;
}

export type PushSupportState =
  | { supported: true; permission: NotificationPermission }
  | { supported: false; reason: string };

export function getPushSupport(): PushSupportState {
  if (typeof window === "undefined") return { supported: false, reason: "不支持浏览器环境" };
  if (isIOS() && !isStandalonePwa()) {
    return {
      supported: false,
      reason: "iOS 要求先点击 Safari 底部「分享 → 添加到主屏幕」，再从主屏图标打开才能免费开启锁屏推送。",
    };
  }
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
    return { supported: false, reason: "当前浏览器不支持 Web Push 推送。" };
  }
  return { supported: true, permission: Notification.permission };
}

function urlBase64ToUint8Array(base64String: string): Uint8Array<ArrayBuffer> {
  const padding = "=".repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const buf = new ArrayBuffer(raw.length);
  const output = new Uint8Array(buf);
  for (let i = 0; i < raw.length; i++) {
    output[i] = raw.charCodeAt(i);
  }
  return output;
}

function arrayBufferToBase64Url(buffer: ArrayBuffer | null): string {
  if (!buffer) return "";
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export const PWA_BUILD_LABEL = "v12";

let swReloadWired = false;

export async function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
  if (!("serviceWorker" in navigator)) return null;
  try {
    const hadController = Boolean(navigator.serviceWorker.controller);
    if (!swReloadWired) {
      swReloadWired = true;
      let reloading = false;
      navigator.serviceWorker.addEventListener("controllerchange", () => {
        if (!hadController || reloading) return;
        reloading = true;
        window.location.reload();
      });
    }
    const reg = await navigator.serviceWorker.register("./sw.js", {
      scope: "./",
      updateViaCache: "none",
    });
    void reg.update().catch(() => {});
    return reg;
  } catch {
    return null;
  }
}

export async function forceRefreshPwa(): Promise<void> {
  try {
    if ("caches" in window) {
      const keys = await caches.keys();
      await Promise.all(keys.map((k) => caches.delete(k)));
    }
    if ("serviceWorker" in navigator) {
      const reg = await navigator.serviceWorker.getRegistration("./");
      if (reg) {
        await reg.update().catch(() => {});
        // Give a newly discovered SW up to 2.5s to install & skipWaiting before reloading.
        const installing = reg.installing || reg.waiting;
        if (installing) {
          await new Promise<void>((resolve) => {
            const timer = window.setTimeout(resolve, 2500);
            const onChange = () => {
              if (installing.state === "activated" || installing.state === "redundant") {
                window.clearTimeout(timer);
                resolve();
              }
            };
            installing.addEventListener("statechange", onChange);
          });
        }
      }
    }
  } finally {
    window.location.reload();
  }
}

export async function getCurrentPushEndpoint(): Promise<string | null> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return null;
  try {
    const reg = await navigator.serviceWorker.ready;
    const sub = await reg.pushManager.getSubscription();
    return sub?.endpoint ?? null;
  } catch {
    return null;
  }
}

export async function enableWebPush(transport: HubTransport): Promise<string> {
  const support = getPushSupport();
  if (!support.supported) throw new Error(support.reason);

  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error("未授予通知权限，请在系统设置中允许 CrossAgents 通知。");
  }

  const keyRes = await transport.request("GET", "/v1/push/vapid-key");
  if (keyRes.status === 404) {
    throw new Error("中心版本偏旧，尚不支持免费 Web Push，请将中心更新到最新版。");
  }
  if (keyRes.status >= 400) {
    throw new Error(`无法读取中心推送公钥（HTTP ${keyRes.status}）`);
  }
  const { public_key } = JSON.parse(keyRes.body) as { public_key: string };

  const reg = await navigator.serviceWorker.ready;
  let sub = await reg.pushManager.getSubscription();
  if (!sub) {
    sub = await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(public_key),
    });
  }

  const p256dh = arrayBufferToBase64Url(sub.getKey("p256dh"));
  const auth = arrayBufferToBase64Url(sub.getKey("auth"));
  const saveRes = await transport.request("POST", "/v1/push/subscriptions", {
    endpoint: sub.endpoint,
    p256dh,
    auth,
  });
  if (saveRes.status >= 400) {
    throw new Error(`向中心注册推送订阅失败（HTTP ${saveRes.status}）`);
  }
  return sub.endpoint;
}

export async function disableWebPush(transport: HubTransport | null): Promise<void> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return;
  const reg = await navigator.serviceWorker.ready;
  const sub = await reg.pushManager.getSubscription();
  if (!sub) return;
  const endpoint = sub.endpoint;
  await sub.unsubscribe().catch(() => {});
  if (transport) {
    await transport.request("DELETE", "/v1/push/subscriptions", { endpoint }).catch(() => {});
  }
}

export async function sendTestPush(transport: HubTransport, endpoint?: string | null): Promise<{ sent: number; failed: number }> {
  const res = await transport.request("POST", "/v1/push/test", endpoint ? { endpoint } : {});
  if (res.status >= 400) {
    let msg = `发送测试推送失败（HTTP ${res.status}）`;
    try {
      const err = JSON.parse(res.body) as { error?: { message?: string } };
      if (err.error?.message) msg = err.error.message;
    } catch {
      // ignore
    }
    throw new Error(msg);
  }
  return JSON.parse(res.body) as { sent: number; failed: number };
}
