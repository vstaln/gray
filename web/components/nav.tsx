import Link from "next/link";
import { site } from "@/lib/site";

const links = [
  { href: "/docs", label: "Docs" },
  { href: "/#panels", label: "Features" },
  { href: "/#pricing", label: "Pricing" },
  { href: site.repo, label: "GitHub", external: true },
];

export function Nav() {
  return (
    <header className="fixed inset-x-0 top-0 z-50 border-b border-ink-700/60 bg-ink-950/70 backdrop-blur-xl">
      <nav className="mx-auto flex h-14 max-w-[1200px] items-center justify-between px-6">
        <Link href="/" className="group flex items-baseline gap-2">
          <span className="font-display text-2xl leading-none">gray</span>
          <span className="text-sand-400 transition-opacity group-hover:opacity-60">.</span>
        </Link>

        <div className="hidden items-center gap-8 md:flex">
          {links.map((l) => (
            <Link
              key={l.label}
              href={l.href}
              {...(l.external ? { target: "_blank", rel: "noreferrer" } : {})}
              className="eyebrow transition-colors hover:text-paper"
            >
              {l.label}
            </Link>
          ))}
        </div>

        <Link
          href="/#install"
          className="eyebrow border border-sand-400/40 bg-sand-400/10 px-4 py-2 text-sand-300 transition-colors hover:bg-sand-400 hover:text-ink-950"
        >
          Install
        </Link>
      </nav>
    </header>
  );
}
