import type { Metadata } from "next";
import { AppIcon } from "@/components/app-icon";
import { Footer } from "@/components/footer";
import { Nav } from "@/components/nav";

// OpenProof (JamesHuckle/OpenProof): not packaged for download yet, so this page describes it.
export const metadata: Metadata = {
  title: "OpenProof: spelling and grammar in every app",
  description: "Underlines spelling and grammar mistakes as you write, in any Windows app. Never rewrites your wording.",
};

const POINTS = [
  {
    title: "Mistakes only",
    body: "Spelling, grammar, punctuation and capitals. Your wording is left alone: no suggestions to sound clearer, shorter or more formal.",
  },
  {
    title: "In every app",
    body: "Gmail, Slack, WhatsApp, Word, Teams and the rest. Mistakes are underlined in place; right-click one to see the fix, then accept or ignore it.",
  },
  {
    title: "Never in the way",
    body: "Each sentence is checked as you finish it, in the background. A small badge on the text box shows the count; Ctrl+Alt+Q lists every fix and Enter accepts them all.",
  },
  {
    title: "Made for OpenWhisper",
    body: "Dictate with OpenWhisper, then OpenProof tidies the result. Right-click the badge to have the whole message rephrased when you want it.",
  },
];

export default function OpenProofPage() {
  return (
    <>
      <Nav current="openproof" />
      <main className="px-6 pb-24 pt-32">
        <div className="mx-auto max-w-2xl">
          <div className="flex items-center gap-4">
            <AppIcon id="openproof" className="h-14 w-14 shrink-0" />
            <div>
              <h1 className="text-3xl font-bold tracking-tight sm:text-4xl">OpenProof</h1>
              <p className="mt-1 text-text-muted">Windows · coming soon</p>
            </div>
          </div>

          <p className="mt-8 text-lg leading-relaxed text-text-muted">
            Spelling and grammar checked as you write, in every app you write in. Like Grammarly, but it only fixes{" "}
            <span className="text-text">mistakes</span>, never the way you put things.
          </p>

          <div className="mt-8 inline-flex items-center gap-2 rounded-xl border border-border px-5 py-3 text-text-muted">
            <span className="h-2 w-2 rounded-full bg-[#7aa2ff]" />
            Download coming soon
          </div>

          <ul className="mt-14 grid gap-8 sm:grid-cols-2">
            {POINTS.map((p) => (
              <li key={p.title}>
                <h2 className="font-semibold">{p.title}</h2>
                <p className="mt-1 leading-relaxed text-text-muted">{p.body}</p>
              </li>
            ))}
          </ul>

          <p className="mt-14 text-sm leading-relaxed text-text-muted/80">
            Checks go to Google&apos;s Gemini (or any OpenAI-compatible model you choose), one sentence at a time. Password
            fields are skipped.
          </p>
        </div>
      </main>
      <Footer />
    </>
  );
}
