import type { Metadata } from "next";
import "./globals.css";

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
    <html lang="en" className="dark">
      <head>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link
          rel="preconnect"
          href="https://fonts.gstatic.com"
          crossOrigin="anonymous"
        />
        <link
          href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700;800&display=swap"
          rel="stylesheet"
        />
      </head>
      <body className="min-h-screen antialiased">{children}</body>
    </html>
  );
}
