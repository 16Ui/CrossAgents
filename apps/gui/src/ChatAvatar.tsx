import { UserRound } from "lucide-react";

export function ChatAvatar({ image, size = 30 }: { image: string | null; size?: number }) {
  const src = image ? (image.startsWith("data:") ? image : `data:image/webp;base64,${image}`) : null;
  return (
    <span className="avatar chat-avatar" style={{ width: size, height: size }} aria-hidden="true">
      {src ? (
        <img className="avatar-img" src={src} alt="" draggable={false} />
      ) : <UserRound size={Math.round(size * 0.55)} strokeWidth={1.7} />}
    </span>
  );
}
