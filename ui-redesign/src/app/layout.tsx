import type { Metadata } from "next";
import type { ReactNode } from "react";
import { MotionProvider } from "@/components/motion-provider";
import "./globals.css";

export const metadata: Metadata = {
  title: "gray. A minimal agent harness in one binary.",
  description:
    "gray runs tools, edits code, and streams from any model provider. Rust, OpenAI-compatible, MIT. No runtime, no node_modules, no dashboard.",
  metadataBase: new URL("https://gray.alignment.id"),
  openGraph: {
    title: "gray. A minimal agent harness in one binary.",
    description:
      "Runs tools, edits code, streams from any provider. Rust, statically linked, MIT.",
    images: ["/space/hero.png"],
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "gray. A minimal agent harness in one binary.",
    description: "Runs tools, edits code, streams from any provider. Rust, statically linked, MIT.",
    images: ["/space/hero.png"],
  },
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className="bg-ink-950">
      <body className="grain min-h-[100dvh] bg-ink-950 text-ink-100 antialiased">
        <a
          href="#main"
          className="focus-ring sr-only left-4 top-4 z-[60] rounded-sm bg-accent px-3 py-2 text-sm font-medium text-ink-950 focus:not-sr-only focus:fixed"
        >
          Skip to content
        </a>
        <MotionProvider>{children}</MotionProvider>
      </body>
    </html>
  );
}
