import { FloatingMic } from "./floating-mic";
import { DownloadButton } from "./download-button";
import { GITHUB_URL } from "@/lib/config";
import { getLatestVersion } from "@/lib/github";
import { GitHubIcon } from "./icons";

export async function Hero() {
  const version = await getLatestVersion();
  return (
    <section className="relative flex min-h-screen flex-col items-center justify-center overflow-hidden px-6 pt-20 pb-32">
      {/* Background gradient */}
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_center,var(--color-accent-glow)_0%,transparent_70%)]" />

      <div className="relative z-10 flex max-w-3xl flex-col items-center text-center">
        {/* Badge */}
        <div className="animate-fade-in-up mb-8 inline-flex items-center gap-2 rounded-full border border-border bg-bg-elevated/60 px-4 py-1.5 text-sm text-text-muted backdrop-blur-sm">
          <span className="h-2 w-2 rounded-full bg-emerald-400" />
          Open source &middot; Free forever
        </div>

        {/* Floating mic demo */}
        <div className="animate-fade-in-up delay-100 mb-10">
          <FloatingMic />
        </div>

        {/* Headline */}
        <h1 className="animate-fade-in-up delay-200 text-5xl font-extrabold leading-[1.1] tracking-tight sm:text-6xl md:text-7xl">
          Voice to text,
          <br />
          <span className="bg-gradient-to-r from-accent to-purple-400 bg-clip-text text-transparent">
            everywhere.
          </span>
        </h1>

        {/* Subtitle */}
        <p className="animate-fade-in-up delay-300 mt-6 max-w-xl text-lg leading-relaxed text-text-muted sm:text-xl">
          A small floating pill that lives on your desktop. Hold{" "}
          <kbd className="rounded border border-border bg-bg-elevated px-1.5 py-0.5 font-mono text-sm">
            Ctrl+Space
          </kbd>
          , speak, release — your words appear instantly in Gmail, Slack, or
          any app you&apos;re in.
        </p>

        {/* OpenAI key callout */}
        <div className="animate-fade-in-up delay-400 mt-6 inline-flex items-center gap-2 rounded-full border border-border bg-bg-elevated/60 px-4 py-2 text-sm text-text-muted backdrop-blur-sm">
          <span className="text-accent">✦</span>
          Only requires an{" "}
          <a
            href="https://platform.openai.com/api-keys"
            target="_blank"
            rel="noopener noreferrer"
            className="font-medium text-text underline-offset-2 hover:underline"
          >
            OpenAI API key
          </a>{" "}
          — takes 30 seconds to create, free to start.
        </div>

        {/* CTA */}
        <div className="animate-fade-in-up delay-500 mt-10 flex flex-col items-center gap-4 sm:flex-row">
          <DownloadButton />
          <a
            href={GITHUB_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 rounded-xl border border-border px-6 py-4 text-sm font-medium text-text-muted transition-colors hover:border-text-muted hover:text-text"
          >
            <GitHubIcon className="h-5 w-5" />
            View on GitHub
          </a>
        </div>

        {/* Version note */}
        <p className="animate-fade-in-up delay-600 mt-6 text-xs text-text-muted/60">
          Windows 10+ &middot; {version ?? "latest"} &middot; ~80 MB
        </p>
      </div>
    </section>
  );
}
