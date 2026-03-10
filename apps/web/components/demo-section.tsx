"use client";

import { useState, useEffect, useRef, useCallback } from "react";
import { AudioVisualizer } from "./audio-visualizer";

const DEMO_APPS = [
  {
    name: "Gmail",
    icon: "📧",
    text: "Hi Sarah, thanks for your email. I'll review the proposal this afternoon and send my feedback by end of day. Looking forward to connecting soon.",
  },
  {
    name: "Slack",
    icon: "💬",
    text: "Hey team, just reviewed the latest PR and everything looks good. Let's ship it today — nice work everyone!",
  },
  {
    name: "Notion",
    icon: "📝",
    text: "Meeting notes: we agreed to push the release to next Friday and focus on bug fixes and polish this week. Alex to lead the QA pass.",
  },
  {
    name: "Word",
    icon: "📄",
    text: "The Q3 review shows a 23% increase in customer satisfaction scores compared to last year, driven by improved support response times.",
  },
  {
    name: "VS Code",
    icon: "🧑‍💻",
    text: "TODO: refactor the auth middleware to use JWT tokens instead of session-based auth for better scalability across services.",
  },
];

const WAVE_HEIGHTS = [4, 8, 11, 7, 10, 5, 8];

type Phase = "idle" | "recording" | "done";

export function DemoSection() {
  const [activeApp, setActiveApp] = useState(0);
  const [phase, setPhase] = useState<Phase>("idle");
  const [typed, setTyped] = useState("");
  const [audioStream, setAudioStream] = useState<MediaStream | null>(null);
  const typingRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      audioStream?.getTracks().forEach((t) => t.stop());
    };
  }, [audioStream]);

  const handleMicClick = useCallback(async () => {
    if (audioStream) {
      audioStream.getTracks().forEach((t) => t.stop());
      setAudioStream(null);
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      setAudioStream(stream);
    } catch {
      // Mic permission denied
    }
  }, [audioStream]);

  const runDemo = useCallback((appIndex: number) => {
    if (typingRef.current) clearInterval(typingRef.current);
    if (timeoutRef.current) clearTimeout(timeoutRef.current);

    setActiveApp(appIndex);
    setPhase("idle");
    setTyped("");

    timeoutRef.current = setTimeout(() => {
      setPhase("recording");
      const text = DEMO_APPS[appIndex].text;
      let i = 0;
      typingRef.current = setInterval(() => {
        i++;
        setTyped(text.slice(0, i));
        if (i >= text.length) {
          clearInterval(typingRef.current!);
          timeoutRef.current = setTimeout(() => setPhase("done"), 300);
        }
      }, 15);
    }, 1200);
  }, []);

  useEffect(() => {
    runDemo(0);
    return () => {
      if (typingRef.current) clearInterval(typingRef.current);
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
    };
  }, [runDemo]);

  // Loop to next app when demo completes
  useEffect(() => {
    if (phase !== "done") return;
    const t = setTimeout(
      () => runDemo((activeApp + 1) % DEMO_APPS.length),
      2500
    );
    return () => clearTimeout(t);
  }, [phase, activeApp, runDemo]);

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

        {/* App selector */}
        <div className="mb-8 flex flex-wrap items-center justify-center gap-3">
          {DEMO_APPS.map((app, i) => (
            <button
              key={app.name}
              onClick={() => runDemo(i)}
              className={`flex cursor-pointer items-center gap-2 rounded-full border px-4 py-2 text-sm transition-all ${
                i === activeApp
                  ? "border-accent/50 bg-accent/10 text-text"
                  : "border-border bg-bg-elevated text-text-muted hover:border-accent/30 hover:text-text"
              }`}
            >
              <span>{app.icon}</span>
              {app.name}
            </button>
          ))}
        </div>

        {/* Demo window */}
        <div className="relative mx-auto max-w-2xl">
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
            <div className="relative min-h-[160px] p-6">
              <div className="text-sm leading-relaxed text-text-muted">
                {typed}
                {phase === "recording" && (
                  <span className="ml-0.5 inline-block h-4 w-0.5 animate-pulse bg-accent" />
                )}
              </div>

            </div>
          </div>

          {/* Floating pill overlay — matches actual app */}
          <div className="absolute left-1/2 -bottom-5 -translate-x-1/2 flex items-center gap-2.5">
            <div
              role="button"
              tabIndex={0}
              onClick={handleMicClick}
              onKeyDown={(e) => e.key === "Enter" && handleMicClick()}
              className={`
                flex cursor-pointer items-center gap-2 rounded-full px-3 py-2
                shadow-xl transition-all duration-500
                ${
                  phase === "recording" || audioStream
                    ? "bg-accent shadow-accent/30 scale-105"
                    : "border border-border bg-bg-elevated shadow-black/30"
                }
              `}
            >
              <svg
                className={`h-3.5 w-3.5 flex-shrink-0 transition-colors ${
                  phase === "recording" || audioStream ? "text-black" : "text-text-muted"
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
              {phase === "recording" || audioStream ? (
                audioStream ? (
                  <div className="flex h-4 items-center">
                    <AudioVisualizer
                      audio={audioStream}
                      width={48}
                      height={24}
                      className="h-6 w-12"
                    />
                  </div>
                ) : (
                  <div className="flex items-center gap-[2px]">
                    {WAVE_HEIGHTS.map((h, i) => (
                      <div
                        key={i}
                        className="w-[2px] rounded-full bg-black/70 animate-wave-bar"
                        style={{ height: `${h}px`, animationDelay: `${i * 0.09}s` }}
                      />
                    ))}
                  </div>
                )
              ) : (
                <span className="text-[11px] font-medium text-text-muted">
                  OpenWhisper
                </span>
              )}
            </div>
            <span className="whitespace-nowrap text-xs text-text-muted">
              Hold{" "}
              <kbd className="rounded border border-border bg-bg-subtle px-1 py-0.5 font-mono text-[11px] text-text">
                Ctrl+Space
              </kbd>{" "}
              to speak
            </span>
          </div>
        </div>
      </div>
    </section>
  );
}
