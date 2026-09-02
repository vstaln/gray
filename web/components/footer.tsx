import Link from "next/link";
import { Backdrop } from "@/components/backdrop";
import { site } from "@/lib/site";

const columns = [
  {
    title: "Product",
    links: [
      { label: "Install", href: "/#install" },
      { label: "Features", href: "/#panels" },
      { label: "Pricing", href: "/#pricing" },
      { label: "Docs", href: "/docs" },
    ],
  },
  {
    title: "Source",
    links: [
      { label: "GitHub", href: site.repo, external: true },
      { label: "Releases", href: `${site.repo}/releases`, external: true },
      { label: "Issues", href: `${site.repo}/issues`, external: true },
      { label: "License — MIT", href: `${site.repo}/blob/main/LICENSE`, external: true },
    ],
  },
  {
    title: "Legal",
    links: [
      { label: "Privacy", href: "/privacy" },
      { label: "Terms", href: "/terms" },
      { label: "Image credits", href: "/credits" },
    ],
  },
];

export function Footer() {
  return (
    <footer className="relative isolate overflow-hidden border-t border-ink-700">
      <Backdrop src="/space/pillars-plate.jpg" opacity={0.3} blur={1} fade="top" />

      <div className="relative mx-auto max-w-[1200px] px-6 pb-14 pt-24">
        <div className="grid gap-12 md:grid-cols-[1.4fr_repeat(3,1fr)]">
          <div>
            <p className="font-display text-4xl">
              gray<span className="text-sand-400">.</span>
            </p>
            <p className="mt-3 max-w-[30ch] text-[15px] text-dim">{site.tagline}.</p>
          </div>

          {columns.map((col) => (
            <div key={col.title}>
              <p className="eyebrow">{col.title}</p>
              <ul className="mt-5 space-y-2.5">
                {col.links.map((l) => (
                  <li key={l.label}>
                    <Link
                      href={l.href}
                      {...("external" in l && l.external
                        ? { target: "_blank", rel: "noreferrer" }
                        : {})}
                      className="text-[15px] text-dim transition-colors hover:text-paper"
                    >
                      {l.label}
                    </Link>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>

        <div className="mt-20 flex flex-col gap-3 border-t border-ink-700 pt-6 sm:flex-row sm:items-center sm:justify-between">
          <p className="font-mono text-[11px] text-dimmer">MIT © 2026 vstaln</p>
          <p className="font-mono text-[11px] text-dimmer">
            Imagery: NASA / JPL-Caltech / STScI — public domain
          </p>
        </div>
      </div>
    </footer>
  );
}
