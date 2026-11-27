// Renders a one-time `ca1-...` pairing code as a scannable QR code data URL (`img-src data:` is
// allowed by CSP in index.html), so CrossAgents iOS can scan and pair with one tap.

import { useEffect, useState } from "react";
import QRCode from "qrcode";

export function PairingQr({ code, size = 168 }: { code: string; size?: number }) {
  const [dataUrl, setDataUrl] = useState<string>();
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    QRCode.toDataURL(code, {
      errorCorrectionLevel: "M",
      margin: 2,
      width: size * 2,
      color: { dark: "#111827", light: "#ffffff" },
    })
      .then((url) => {
        if (!cancelled) setDataUrl(url);
      })
      .catch(() => {
        if (!cancelled) {
          setDataUrl(undefined);
          setFailed(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [code, size]);

  if (failed) return null;

  return (
    <div className="pairing-qr-card" data-role="invite-qr">
      {dataUrl ? (
        <img
          className="pairing-qr-img"
          src={dataUrl}
          width={size}
          height={size}
          alt="CrossAgents 配对二维码"
        />
      ) : (
        <div className="pairing-qr-placeholder" style={{ width: size, height: size }}>
          生成二维码…
        </div>
      )}
      <div className="pairing-qr-caption">iPhone 扫码一键配对</div>
    </div>
  );
}
