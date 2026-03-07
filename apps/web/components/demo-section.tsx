"use client";

import { useState, useEffect, useRef } from "react";

const DEMO_APPS = [
  { name: "Slack", icon: "💬" },
  { name: "VS Code", icon: "🧑‍💻" },
  { name: "Gmail", icon: "📧" },
  { name: "Notion", icon: "📝" },
  { name: "Word", icon: "📄" },
];

const DEMO_TEXT =
  "Hey team, I just reviewed the latest pull request and everything looks good. Let's ship it today.";

export function DemoSection() {
  const [phase, setPhase] = useState<
    "idle" | "activated" | "speaking" | "done"
  >("idle");
  const [typed, setTyped] = useState("");
  const [activeApp, setActiveApp] = useState(0);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    const cycle = () => {
      setPhase("idle");
      setTyped("");

      setTimeout(() => setPhase("activated"), 800);
      setTimeout(() => {
        setPhase("speaking");
        let i = 0;
        intervalRef.current = setInterval(() => {
          i++;
          setTyped(DEMO_TEXT.slice(0, i));
          if (i >= DEMO_TEXT.length) {
            if (intervalRef.current) clearInterval(intervalRef.current);
            setTimeout(() => {
              setPhase("done");
              setTimeout(() => {
                setActiveApp((prev) => (prev + 1) % DEMO_APPS.length);
              }, 1500);
            }, 400);
          }
        }, 30);
      }, 1800);
    };

    cycle();
    const loopInterval = setInterval(cycle, 9000);
    return () => {
      clearInterval(loopInterval);
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, []);

  return (
    <section id="demo" className="relative px-6 py-32">
      <div className="mx-auto max-w-5xl">
        <div className="mb-16 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            See it in action
          </h2>
          <p className="mt-4 text-lg text-text-muted">
            Works with every app on your desktop. No plugins, no extensions.
          </p>
        </div>

        {/* Demo window */}
        <div className="relative mx-auto max-w-2xl">
          {/* Fake app window */}
          <div className="overflow-hidden rounded-2xl border border-border bg-bg-elevated shadow-2xl shadow-black/40">
            {/* Title bar */}
            <div className="flex items-center gap-2 border-b border-border px-4 py-3">
              <div className="flex gap-1.5">
                <div className="h-3 w-3 rounded-full bg-red-500/70" />
                <div className="h-3 w-3 rounded-full bg-yellow-500/70" />
                <div className="h-3 w-3 rounded-full bg-green-500/70" />
              </div>
              <div className="flex-1 text-center text-sm text-text-muted">
                <span className="mr-2">{DEMO_APPS[activeApp].icon}</span>
                {DEMO_APPS[activeApp].name}
              </div>
            </div>

            {/* Content area */}
            <div className="relative min-h-[200px] p-6">
              <div className="font-mono text-sm leading-relaxed text-text-muted">
                {typed}
                {phase === "speaking" && (
                  <span className="ml-0.5 inline-block h-4 w-0.5 animate-pulse bg-accent" />
                )}
              </div>

              {phase === "done" && (
                <div className="mt-4 inline-flex items-center gap-1.5 rounded-full bg-emerald-500/10 px-3 py-1 text-xs text-emerald-400">
                  <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                  Inserted
                </div>
              )}
            </div>
          </div>

          {/* Floating mic overlay */}
          <div
            className={`
              absolute -right-4 -bottom-4 flex h-14 w-14 items-center justify-center
              rounded-full shadow-xl transition-all duration-500
              ${
                phase === "speaking" || phase === "activated"
                  ? "bg-accent shadow-accent/30 scale-110"
                  : "bg-bg-subtle shadow-black/30"
              }
            `}
          >
            <svg
              className={`h-6 w-6 transition-colors ${
                phase === "speaking" || phase === "activated"
                  ? "text-white"
                  : "text-text-muted"
              }`}
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
              <line x1="12" x2="12" y1="19" y2="22" />
            </svg>
          </div>

          {/* Keyboard shortcut indicator */}
          <div className="absolute -left-4 -bottom-4 flex items-center gap-2 rounded-xl border border-border bg-bg-elevated px-3 py-2 shadow-xl">
            {phase === "idle" && (
              <span className="text-xs text-text-muted">
                Press{" "}
                <kbd className="rounded border border-border bg-bg-subtle px-1.5 py-0.5 font-mono text-xs text-text">
                  Win+S
                </kbd>
              </span>
            )}
            {phase === "activated" && (
              <span className="text-xs text-accent">Listening...</span>
            )}
            {phase === "speaking" && (
              <span className="text-xs text-accent">
                <span className="inline-block animate-pulse">Recording</span>
              </span>
            )}
            {phase === "done" && (
              <span className="text-xs text-emerald-400">
                <kbd className="rounded border border-border bg-bg-subtle px-1.5 py-0.5 font-mono text-xs text-text">
                  Enter
                </kbd>{" "}
                Done!
              </span>
            )}
          </div>
        </div>

        {/* App icons row */}
        <div className="mt-16 flex flex-wrap items-center justify-center gap-6">
          {DEMO_APPS.map((app, i) => (
            <div
              key={app.name}
              className={`flex items-center gap-2 rounded-full border px-4 py-2 text-sm transition-all ${
                i === activeApp
                  ? "border-accent/50 bg-accent/10 text-text"
                  : "border-border bg-bg-elevated text-text-muted"
              }`}
            >
              <span>{app.icon}</span>
              {app.name}
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
