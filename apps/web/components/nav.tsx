"use client";

import { GITHUB_URL } from "@/lib/config";
import { APPS, type AppId } from "@/lib/apps";
import { GitHubIcon } from "./icons";
import { AppIcon } from "./app-icon";
import { AppsMenu } from "./apps-menu";
import { DownloadButton } from "./download-button";

/** Top bar. On OpenWhisper's page: its sections and download; on the other apps' pages: that app. Always the apps menu. */
export function Nav({ current = "openwhisper" }: { current?: AppId }) {
  const app = APPS.find((a) => a.id === current)!;
  const home = current === "openwhisper";
  return (
    <nav className="fixed top-0 z-50 w-full border-b border-border/50 bg-bg/80 backdrop-blur-xl">
      <div className="mx-auto flex max-w-6xl items-center justify-between gap-4 px-6 py-3">
        <a href={home ? "#" : app.href} className="flex items-center gap-2.5">
          <AppIcon id={current} className="h-8 w-8 shrink-0" />
          <span className="text-base font-semibold tracking-tight">{app.name}</span>
        </a>

        {home && (
          <div className="hidden items-center gap-8 text-sm text-text-muted md:flex">
            <a href="#demo" className="transition-colors hover:text-text">
              Demo
            </a>
            <a href="#how-it-works" className="transition-colors hover:text-text">
              How it works
            </a>
            <a href="#features" className="transition-colors hover:text-text">
              Features
            </a>
            <a href={GITHUB_URL} target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-text">
              <GitHubIcon className="h-5 w-5" />
            </a>
          </div>
        )}

        <div className="flex items-center gap-2">
          {home && (
            <div className="hidden sm:block">
              <DownloadButton size="sm" />
            </div>
          )}
          <AppsMenu current={current} />
        </div>
      </div>
    </nav>
  );
}
