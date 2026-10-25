import { APPS, FAMILY } from "@/lib/apps";
import { AppIcon } from "./app-icon";

/** The three apps together, on OpenWhisper's page. */
export function Family() {
  return (
    <section id="apps" className="relative px-6 py-28">
      <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent/30 to-transparent" />
      <div className="mx-auto max-w-5xl">
        <div className="mb-14 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">{APPS.map((a) => a.verb).join(" · ")}</h2>
          <p className="mx-auto mt-4 max-w-xl text-lg text-text-muted">{FAMILY}</p>
        </div>
        <div className="grid gap-4 sm:grid-cols-3">
          {APPS.map((app) => (
            <a
              key={app.id}
              href={app.id === "openwhisper" ? "#" : app.href}
              className="group flex flex-col rounded-2xl border border-border bg-bg-elevated/60 p-6 transition-colors hover:border-accent/50"
            >
              <AppIcon id={app.id} className="h-12 w-12" />
              <p className="mt-5 text-sm font-medium text-text-muted">{app.verb}</p>
              <h3 className="mt-1 text-xl font-semibold">{app.name}</h3>
              <p className="mt-2 flex-1 leading-relaxed text-text-muted">{app.tagline}</p>
              <p className="mt-5 text-sm text-text-muted/80">
                {app.platforms}
                {app.status === "coming-soon" ? " · coming soon" : ""}
              </p>
            </a>
          ))}
        </div>
      </div>
    </section>
  );
}
