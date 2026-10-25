"use client";

import { useEffect, useId, useRef, useState } from "react";
import { APPS, FAMILY, type AppId } from "@/lib/apps";
import { AppIcon } from "./app-icon";

/** The menu button at the right of the top bar: switch between the three apps. */
export function AppsMenu({ current }: { current: AppId }) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);

  const close = (focusButton = false) => {
    setOpen(false);
    if (focusButton) button.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    const items = () => Array.from(panel.current?.querySelectorAll<HTMLAnchorElement>("a[data-app]") ?? []);
    items()[0]?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(true);
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const list = items();
        const i = list.indexOf(document.activeElement as HTMLAnchorElement);
        list[(i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length]?.focus();
      }
    };
    const onPointer = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!panel.current?.contains(t) && !button.current?.contains(t)) close();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [open]);

  return (
    <div className="relative">
      <button
        ref={button}
        type="button"
        aria-label="Apps"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={() => setOpen((o) => !o)}
        className={`flex h-10 w-10 items-center justify-center rounded-xl border transition-colors ${
          open ? "border-border bg-bg-elevated text-text" : "border-transparent text-text-muted hover:bg-bg-elevated hover:text-text"
        } focus-visible:outline-2 focus-visible:outline-accent`}
      >
        <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" aria-hidden="true">
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </button>

      {open && (
        <div
          ref={panel}
          id={panelId}
          role="dialog"
          aria-label="Apps"
          className="animate-menu-in absolute right-0 top-full z-50 mt-2 w-[22rem] max-w-[calc(100vw-2rem)] origin-top-right rounded-2xl border border-border bg-bg-elevated p-2 shadow-[0_18px_50px_rgba(0,0,0,0.55)]"
        >
          <p className="px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wider text-text-muted">Apps</p>
          <ul>
            {APPS.map((app) => {
              const here = app.id === current;
              return (
                <li key={app.id}>
                  <a
                    href={app.href}
                    data-app={app.id}
                    aria-current={here ? "page" : undefined}
                    onClick={() => close()}
                    className={`flex items-center gap-3 rounded-xl p-3 outline-none transition-colors hover:bg-bg-subtle focus-visible:bg-bg-subtle ${here ? "bg-bg-subtle/60" : ""}`}
                  >
                    <AppIcon id={app.id} className="h-10 w-10 shrink-0" />
                    <span className="min-w-0">
                      <span className="flex items-center gap-2">
                        <span className="font-semibold text-text">{app.name}</span>
                        {app.status === "coming-soon" && (
                          <span className="rounded-full border border-border px-2 py-px text-[11px] text-text-muted">Coming soon</span>
                        )}
                      </span>
                      <span className="block text-sm leading-snug text-text-muted">{app.tagline}</span>
                      <span className="block text-xs text-text-muted/70">{app.platforms}</span>
                    </span>
                  </a>
                </li>
              );
            })}
          </ul>
          <p className="border-t border-border/60 px-3 pb-2 pt-3 text-xs leading-relaxed text-text-muted">{FAMILY}</p>
        </div>
      )}
    </div>
  );
}
