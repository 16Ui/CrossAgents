import { createCipheriv, randomBytes } from "node:crypto";

export function sealTransfer(format: string, payload: unknown): { file: string; key: string } {
  const key = randomBytes(32);
  const nonce = randomBytes(12);
  try {
    const cipher = createCipheriv("aes-256-gcm", key, nonce);
    cipher.setAAD(Buffer.from(format));
    const data = Buffer.concat([cipher.update(JSON.stringify(payload), "utf8"), cipher.final(), cipher.getAuthTag()]);
    return {
      key: key.toString("base64url"),
      file: JSON.stringify({ format, nonce: nonce.toString("base64url"), data: data.toString("base64url") }),
    };
  } finally {
    key.fill(0);
  }
}
