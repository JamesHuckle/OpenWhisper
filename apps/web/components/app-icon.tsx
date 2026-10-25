import type { AppId } from "@/lib/apps";
import { LogoIcon } from "./icons";

/** Each app's icon: the same dark rounded square, its own mark. */
export function AppIcon({ id, className = "" }: { id: AppId; className?: string }) {
  if (id === "openwhisper") return <LogoIcon className={className} />;
  if (id === "zoon-meetings") {
    return (
      <svg viewBox="0 0 512 512" className={className} aria-hidden="true">
        <rect width="512" height="512" rx="96" fill="#1d1d1f" />
        <circle cx="256" cy="256" r="104" fill="#ff3b30" />
      </svg>
    );
  }
  // OpenProof: a tick over a wavy underline.
  return (
    <svg viewBox="0 0 512 512" className={className} aria-hidden="true">
      <rect width="512" height="512" rx="96" fill="#0d1016" />
      <g fill="none" strokeLinecap="round" strokeLinejoin="round">
        <path d="M150 232l70 70 142-150" stroke="#7aa2ff" strokeWidth="36" />
        <path d="M136 372c20-18 40-18 60 0s40 18 60 0 40-18 60 0 40 18 60 0" stroke="#7aa2ff" strokeOpacity="0.55" strokeWidth="20" />
      </g>
    </svg>
  );
}
