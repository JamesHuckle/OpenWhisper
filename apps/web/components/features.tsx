import { ZapIcon, GlobeIcon, LockIcon, MicIcon } from "./icons";

const FEATURES = [
  {
    icon: ZapIcon,
    title: "Blazing fast",
    description:
      "Powered by OpenAI's latest speech models. Transcription starts before you finish speaking.",
  },
  {
    icon: GlobeIcon,
    title: "Works everywhere",
    description:
      "Slack, VS Code, Gmail, Word, Notion — if you can type in it, OpenWhisper works with it.",
  },
  {
    icon: MicIcon,
    title: "Always-on-top mic",
    description:
      "A sleek 80×80 pixel floating orb. Stays out of your way but always within reach.",
  },
  {
    icon: LockIcon,
    title: "Private by design",
    description:
      "Your API key, your data. Audio is sent directly to OpenAI — nothing is stored or logged.",
  },
];

export function Features() {
  return (
    <section id="features" className="relative px-6 py-32">
      {/* Divider glow */}
      <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent/30 to-transparent" />

      <div className="mx-auto max-w-5xl">
        <div className="mb-16 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            Built for speed and simplicity
          </h2>
          <p className="mt-4 text-lg text-text-muted">
            No bloat, no complexity. Just voice-to-text that works.
          </p>
        </div>

        <div className="grid gap-6 sm:grid-cols-2">
          {FEATURES.map((feature) => (
            <div
              key={feature.title}
              className="group rounded-2xl border border-border bg-bg-elevated/30 p-8 transition-all hover:border-accent/20 hover:bg-bg-elevated/60"
            >
              <div className="mb-4 flex h-11 w-11 items-center justify-center rounded-xl bg-accent/10 transition-colors group-hover:bg-accent/20">
                <feature.icon className="h-5 w-5 text-accent" />
              </div>
              <h3 className="mb-2 text-lg font-semibold">{feature.title}</h3>
              <p className="text-sm leading-relaxed text-text-muted">
                {feature.description}
              </p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
