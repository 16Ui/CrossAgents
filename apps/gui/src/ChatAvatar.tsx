import { UserRound } from "lucide-react";

export function ChatAvatar({ image, size = 30 }: { image: string | null; size?: number }) {
  return (
    <span className="avatar chat-avatar" style={{ width: size, height: size }} aria-hidden="true">
      {image ? (
        <img className="avatar-img" src={`data:image/webp;base64,${image}`} alt="" draggable={false} />
      ) : <UserRound size={Math.round(size * 0.55)} strokeWidth={1.7} />}
    </span>
  );
}
