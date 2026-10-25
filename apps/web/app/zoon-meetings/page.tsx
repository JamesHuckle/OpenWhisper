import { readFileSync } from "node:fs";
import path from "node:path";
import type { Metadata } from "next";
import Link from "next/link";
import { Footer } from "@/components/footer";
import { AndroidIcon, DownloadIcon } from "@/components/icons";

// Zoon Meetings for Android: built in JamesHuckle/zoon-meetings (android/release.sh), published
// here with its update feed. The app checks /zoon-meetings/android-update.json once a day.
export const metadata: Metadata = {
  title: "Zoon Meetings for Android",
  description: "Asks “Record this call?” when a call connects, and files the notes in Zoon Meetings.",
  robots: { index: false, follow: false },
};

const APK = "/zoon-meetings/ZoonMeetings-Android.apk";

type Release = { versionCode: number; versionName: string; apk: string; sha256: string };

function release(): Release {
  const file = path.join(process.cwd(), "public/zoon-meetings/android-update.json");
  return JSON.parse(readFileSync(file, "utf8"));
}

const STEPS = [
  {
    title: "Download and open the APK",
    body: "Tap Download on the phone, then open the file. If Samsung blocks it, turn off Auto Blocker (Settings → Security and privacy) for the install and turn it back on afterwards.",
  },
  {
    title: "Follow the steps in the app",
    body: "Sign in with Google, then allow the microphone and notifications, and turn on Notice calls and Record during calls. The app shows each step in order.",
  },
  {
    title: "Switch greyed out?",
    body: "Android restricts these settings for apps installed outside the Play Store. Open App info, tap ⋮, choose Allow restricted settings, then try again.",
  },
  {
    title: "Phone calls with both sides (optional)",
    body: "In Samsung's Phone app, turn on Settings → Record calls → Auto record calls, then tap Allow in Zoon Meetings. Calls you choose to record then upload Samsung's recording, which has both sides.",
  },
];

export default function ZoonMeetingsPage() {
  const r = release();
  return (
    <>
      <main className="px-6 pb-24 pt-20">
        <div className="mx-auto max-w-2xl">
          <Link href="/" className="text-sm text-text-muted transition-colors hover:text-text">
            ← OpenWhisper
          </Link>

          <div className="mt-10 flex items-center gap-4">
            <svg viewBox="0 0 512 512" className="h-14 w-14 shrink-0" aria-hidden="true">
              <rect width="512" height="512" rx="112" fill="#1d1d1f" />
              <circle cx="256" cy="256" r="104" fill="#ff3b30" />
            </svg>
            <div>
              <h1 className="text-3xl font-bold tracking-tight sm:text-4xl">Zoon Meetings for Android</h1>
              <p className="mt-1 text-text-muted">Version {r.versionName} · Android 13 or later</p>
            </div>
          </div>

          <p className="mt-8 text-lg leading-relaxed text-text-muted">
            When a WhatsApp, Teams, Meet, Zoom or phone call connects, Zoon Meetings asks{" "}
            <span className="text-text">“Record this call?”</span>. Tap Record and the call is transcribed, summarised and
            filed in your calendar, like the desktop app does.
          </p>

          <a
            href={APK}
            download
            className="mt-8 inline-flex items-center justify-center gap-3 rounded-xl bg-accent px-8 py-4 text-lg font-semibold text-black transition-all duration-200 hover:bg-accent-hover hover:shadow-[0_0_40px_var(--color-accent-glow)] active:scale-[0.98]"
          >
            <DownloadIcon className="h-5 w-5 text-black" />
            Download for Android
            <AndroidIcon className="h-5 w-5 text-black/60" />
          </a>
          <p className="mt-3 text-sm text-text-muted">
            Private beta: it signs in only to its own Zoon Meetings account. Updates are announced in the app.
          </p>

          <ol className="mt-14 space-y-6">
            {STEPS.map((s, i) => (
              <li key={s.title} className="flex gap-4">
                <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-bg-subtle text-sm font-semibold">
                  {i + 1}
                </span>
                <div>
                  <h2 className="font-semibold">{s.title}</h2>
                  <p className="mt-1 leading-relaxed text-text-muted">{s.body}</p>
                </div>
              </li>
            ))}
          </ol>

          <p className="mt-14 break-all text-xs text-text-muted/70">SHA-256 {r.sha256}</p>
        </div>
      </main>
      <Footer />
    </>
  );
}
