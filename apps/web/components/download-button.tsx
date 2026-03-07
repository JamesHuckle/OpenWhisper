"use client";

import { DOWNLOAD_URL } from "@/lib/config";
import { DownloadIcon, WindowsIcon } from "./icons";

export function DownloadButton({ size = "lg" }: { size?: "lg" | "sm" }) {
  const isLarge = size === "lg";

  return (
    <a
      href={DOWNLOAD_URL}
      className={`
        group relative inline-flex items-center justify-center gap-3
        rounded-xl bg-accent font-semibold text-white
        transition-all duration-200
        hover:bg-accent-hover hover:shadow-[0_0_40px_var(--color-accent-glow)]
        active:scale-[0.98]
        ${isLarge ? "px-8 py-4 text-lg" : "px-6 py-3 text-base"}
      `}
    >
      <DownloadIcon className="h-5 w-5" />
      Download for Windows
      <WindowsIcon className="h-4 w-4 opacity-60" />
    </a>
  );
}
