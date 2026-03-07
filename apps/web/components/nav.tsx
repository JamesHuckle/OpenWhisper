"use client";

import { GITHUB_URL } from "@/lib/config";
import { GitHubIcon, LogoIcon } from "./icons";
import { DownloadButton } from "./download-button";

export function Nav() {
  return (
    <nav className="fixed top-0 z-50 w-full border-b border-border/50 bg-bg/80 backdrop-blur-xl">
      <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-3">
        <a href="#" className="flex items-center gap-2.5">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-accent">
            <LogoIcon className="h-5 w-5 text-white" />
          </div>
          <span className="text-base font-semibold tracking-tight">
            OpenWhisper
          </span>
        </a>

        <div className="hidden items-center gap-8 text-sm text-text-muted md:flex">
          <a
            href="#demo"
            className="transition-colors hover:text-text"
          >
            Demo
          </a>
          <a
            href="#how-it-works"
            className="transition-colors hover:text-text"
          >
            How it works
          </a>
          <a
            href="#features"
            className="transition-colors hover:text-text"
          >
            Features
          </a>
          <a
            href={GITHUB_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="transition-colors hover:text-text"
          >
            <GitHubIcon className="h-5 w-5" />
          </a>
        </div>

        <div className="hidden sm:block">
          <DownloadButton size="sm" />
        </div>
      </div>
    </nav>
  );
}
