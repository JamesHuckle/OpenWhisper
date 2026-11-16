import type { Metadata } from "next";
import { Inter } from "next/font/google";
import "./globals.css";

// Next downloads Inter at build time and serves it from this site, so text
// shows without waiting on Google's servers and visitors' IPs stay with us.
const inter = Inter({
  subsets: ["latin"],
  weight: ["400", "500", "600", "700", "800"],
  display: "swap",
  variable: "--font-inter",
});

export const metadata: Metadata = {
  title: "OpenWhisper — Voice to text, everywhere",
  description:
    "A tiny floating microphone that transcribes your speech in real-time and pastes it into any app. Press Win+S to start, Enter to finish. Powered by OpenAI.",
  openGraph: {
    title: "OpenWhisper — Voice to text, everywhere",
    description:
      "A tiny floating microphone that transcribes your speech in real-time and pastes it into any app.",
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "OpenWhisper — Voice to text, everywhere",
    description:
      "A tiny floating microphone that transcribes your speech in real-time and pastes it into any app.",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className={`dark ${inter.variable}`}>
      <body className="min-h-screen antialiased">{children}</body>
    </html>
  );
}
