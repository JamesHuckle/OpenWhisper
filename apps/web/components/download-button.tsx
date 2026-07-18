"use client";

import { ANDROID_DOWNLOAD_URL, WINDOWS_DOWNLOAD_URL } from "@/lib/config";
import { AndroidIcon, DownloadIcon, WindowsIcon } from "./icons";

export function DownloadButton({
  size = "lg",
  platform = "windows",
}: {
  size?: "lg" | "sm";
  platform?: "windows" | "android";
}) {
  const isLarge = size === "lg";
  const isAndroid = platform === "android";
  const href = isAndroid ? ANDROID_DOWNLOAD_URL : WINDOWS_DOWNLOAD_URL;
  const label = isAndroid ? "Download for Android" : "Download for Windows";

  return (
    <a
      href={href}
      className={`
        group relative inline-flex items-center justify-center gap-3
        rounded-xl font-semibold
        transition-all duration-200
        active:scale-[0.98]
        ${
          isAndroid
            ? "border border-accent/60 bg-bg-elevated text-text hover:border-accent hover:bg-accent/10 hover:shadow-[0_0_40px_var(--color-accent-glow)]"
            : "bg-accent text-black hover:bg-accent-hover hover:shadow-[0_0_40px_var(--color-accent-glow)]"
        }
        ${isLarge ? "px-8 py-4 text-lg" : "px-6 py-3 text-base"}
      `}
    >
      <DownloadIcon className={`h-5 w-5 ${isAndroid ? "text-accent" : "text-black"}`} />
      {label}
      {isAndroid ? (
        <AndroidIcon className="h-5 w-5 text-accent" />
      ) : (
        <WindowsIcon className="h-4 w-4 text-black/60" />
      )}
    </a>
  );
}
