import { useEffect, useRef, useState } from "react";
import jsQR from "jsqr";
import { X } from "lucide-react";

export function QrScannerModal({
  onScanned,
  onClose,
}: {
  onScanned: (code: string) => void;
  onClose: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [hint, setHint] = useState("将电脑端 CrossAgents 的配对二维码置于框内");
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    let stream: MediaStream | null = null;
    let rafId = 0;

    async function start() {
      if (!navigator.mediaDevices?.getUserMedia) {
        setError("当前浏览器不支持相机扫码（请确认在 HTTPS 页面下打开），或返回手动粘贴配对码。");
        return;
      }
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: { ideal: "environment" } },
          audio: false,
        });
        if (!active) {
          stream.getTracks().forEach((t) => t.stop());
          return;
        }
        const video = videoRef.current;
        if (!video) return;
        video.srcObject = stream;
        await video.play();
        tick();
      } catch (err) {
        setError(
          `无法打开相机：${err instanceof Error ? err.message : String(err)}。你也可以返回直接粘贴 ca1-... 配对码。`,
        );
      }
    }

    function tick() {
      if (!active) return;
      const video = videoRef.current;
      const canvas = canvasRef.current;
      if (video && canvas && video.readyState === video.HAVE_ENOUGH_DATA && video.videoWidth > 0) {
        const w = Math.min(video.videoWidth, 640);
        const h = Math.round((video.videoHeight / video.videoWidth) * w);
        canvas.width = w;
        canvas.height = h;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        if (ctx) {
          ctx.drawImage(video, 0, 0, w, h);
          const imgData = ctx.getImageData(0, 0, w, h);
          const found = jsQR(imgData.data, imgData.width, imgData.height, {
            inversionAttempts: "dontInvert",
          });
          if (found?.data) {
            const text = found.data.trim();
            if (text.startsWith("ca1-")) {
              active = false;
              navigator.vibrate?.(30);
              onScanned(text);
              return;
            }
            setHint("扫到的不是 CrossAgents 配对码（应以 ca1- 开头）");
          }
        }
      }
      rafId = requestAnimationFrame(tick);
    }

    void start();
    return () => {
      active = false;
      cancelAnimationFrame(rafId);
      stream?.getTracks().forEach((t) => t.stop());
    };
  }, [onScanned]);

  return (
    <div className="qr-fullscreen">
      <div
        className="nav-bar"
        style={{ background: "rgba(0,0,0,0.7)", color: "#fff", borderColor: "rgba(255,255,255,0.12)" }}
      >
        <button type="button" className="icon-btn" style={{ background: "rgba(255,255,255,0.16)", color: "#fff" }} onClick={onClose}>
          <X size={20} />
        </button>
        <div className="nav-title-wrap">
          <h1 className="nav-title" style={{ color: "#fff" }}>
            扫码配对
          </h1>
        </div>
      </div>

      <div className="qr-video-wrap">
        {error ? (
          <div className="empty-state" style={{ color: "#e5e7eb", marginTop: 64 }}>
            <h3 style={{ color: "#fff" }}>无法调用相机</h3>
            <p>{error}</p>
            <button type="button" className="primary-btn" style={{ marginTop: 18, maxWidth: 240 }} onClick={onClose}>
              返回粘贴配对码
            </button>
          </div>
        ) : (
          <>
            <video ref={videoRef} playsInline muted />
            <canvas ref={canvasRef} style={{ display: "none" }} />
            <div className="qr-frame" />
            <div
              style={{
                position: "absolute",
                bottom: 54,
                left: 20,
                right: 20,
                textAlign: "center",
              }}
            >
              <span
                style={{
                  display: "inline-block",
                  padding: "8px 16px",
                  borderRadius: 999,
                  background: "rgba(0,0,0,0.68)",
                  color: "#fff",
                  fontSize: 13,
                }}
              >
                {hint}
              </span>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
