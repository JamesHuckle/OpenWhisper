// The three apps the site offers, in the apps menu, on the homepage and on each app's page.
export type AppId = "openwhisper" | "zoon-meetings" | "openproof";

export type AppInfo = {
  id: AppId;
  name: string;
  href: string;
  /** What it does, in a few words: the menu line. */
  tagline: string;
  /** Where it runs. */
  platforms: string;
  /** One verb for the "Speak it · Record it · Proofread it" line. */
  verb: string;
  status?: "coming-soon";
};

export const APPS: AppInfo[] = [
  {
    id: "openwhisper",
    name: "OpenWhisper",
    href: "/",
    tagline: "Speak, and your words appear in any app.",
    platforms: "Windows · Android",
    verb: "Speak it",
  },
  {
    id: "zoon-meetings",
    name: "Zoon Meetings",
    href: "/zoon-meetings",
    tagline: "Records your calls and writes the notes.",
    platforms: "Android",
    verb: "Record it",
  },
  {
    id: "openproof",
    name: "OpenProof",
    href: "/openproof",
    tagline: "Spelling and grammar checked in every app.",
    platforms: "Windows",
    verb: "Proofread it",
    status: "coming-soon",
  },
];

/** One sentence for the set. */
export const FAMILY = "Three small apps that work together: speak it, record it, proofread it.";
