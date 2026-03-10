import { GITHUB_URL, RELEASES_URL } from "@/lib/config";
import { LogoIcon, GitHubIcon } from "./icons";

export function Footer() {
  return (
    <footer className="border-t border-border px-6 py-12">
      <div className="mx-auto flex max-w-5xl flex-col items-center gap-6 sm:flex-row sm:justify-between">
        <div className="flex items-center gap-2.5">
          <LogoIcon className="h-7 w-7 shrink-0" />
          <span className="text-sm text-text-muted">
            OpenWhisper &middot; Open source voice-to-text
          </span>
        </div>

        <div className="flex items-center gap-6 text-sm text-text-muted">
          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="transition-colors hover:text-text"
          >
            Releases
          </a>
          <a
            href={GITHUB_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1.5 transition-colors hover:text-text"
          >
            <GitHubIcon className="h-4 w-4" />
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
