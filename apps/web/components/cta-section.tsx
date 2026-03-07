import { DownloadButton } from "./download-button";
import { GITHUB_URL } from "@/lib/config";
import { GitHubIcon } from "./icons";

export function CTASection() {
  return (
    <section className="relative px-6 py-32">
      <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent/30 to-transparent" />

      <div className="mx-auto max-w-2xl text-center">
        <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
          Stop typing. Start talking.
        </h2>
        <p className="mt-4 text-lg text-text-muted">
          Download OpenWhisper and turn your voice into text in any app,
          instantly.
        </p>

        <div className="mt-10 flex flex-col items-center gap-4 sm:flex-row sm:justify-center">
          <DownloadButton />
          <a
            href={GITHUB_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 rounded-xl border border-border px-6 py-4 text-sm font-medium text-text-muted transition-colors hover:border-text-muted hover:text-text"
          >
            <GitHubIcon className="h-5 w-5" />
            Star on GitHub
          </a>
        </div>
      </div>
    </section>
  );
}
