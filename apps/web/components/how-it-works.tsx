import { KeyboardIcon, MicIcon, ZapIcon } from "./icons";

const STEPS = [
  {
    icon: KeyboardIcon,
    shortcut: "Ctrl + Space",
    title: "Hold to record",
    description:
      "Hold Ctrl+Space from any app. A small floating pill appears and starts listening immediately.",
  },
  {
    icon: MicIcon,
    shortcut: null,
    title: "Speak",
    description:
      "Talk naturally. OpenAI's speech models transcribe your words in real-time with incredible accuracy.",
  },
  {
    icon: ZapIcon,
    shortcut: "Release",
    title: "Done",
    description:
      "Let go of Ctrl+Space and the transcribed text is instantly pasted into whatever app you're in.",
  },
];

export function HowItWorks() {
  return (
    <section id="how-it-works" className="relative px-6 py-32">
      <div className="mx-auto max-w-5xl">
        <div className="mb-16 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            Three seconds to start
          </h2>
          <p className="mt-4 text-lg text-text-muted">
            No setup, no learning curve. It just works.
          </p>
        </div>

        <div className="grid gap-8 md:grid-cols-3">
          {STEPS.map((step, i) => (
            <div
              key={step.title}
              className="group relative rounded-2xl border border-border bg-bg-elevated/50 p-8 transition-all hover:border-accent/30 hover:bg-bg-elevated"
            >
              {/* Step number */}
              <div className="absolute -top-3 left-6 flex h-7 w-7 items-center justify-center rounded-full bg-accent text-xs font-bold text-black">
                {i + 1}
              </div>

              <div className="mb-5 flex items-center gap-3">
                <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-accent/10">
                  <step.icon className="h-6 w-6 text-accent" />
                </div>
                {step.shortcut && (
                  <kbd className="rounded-lg border border-border bg-bg-subtle px-3 py-1.5 font-mono text-sm text-text">
                    {step.shortcut}
                  </kbd>
                )}
              </div>

              <h3 className="mb-2 text-xl font-semibold">{step.title}</h3>
              <p className="text-sm leading-relaxed text-text-muted">
                {step.description}
              </p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
