import Link from "next/link";
import type { Metadata } from "next";
import { docsBySection } from "@/lib/docs";

export const metadata: Metadata = {
  title: "Docs",
  description: "Documentation for gray — install, configure, and run the agent.",
};

export default function DocsIndex() {
  const groups = docsBySection();

  return (
    <>
      <p className="eyebrow">Documentation</p>
      <h1 className="display mt-4 text-[clamp(2.5rem,5vw,4rem)]">Everything gray does.</h1>
      <p className="mt-5 max-w-[62ch] text-dim">
        Every page here describes behavior that exists in the source. Where a page documents a
        surface, it names the crate it came from.
      </p>

      <div className="mt-14 space-y-12">
        {groups.map((g) => (
          <section key={g.section}>
            <p className="eyebrow border-b border-ink-700 pb-3">{g.section}</p>
            <ul className="mt-5 grid gap-px bg-ink-700 sm:grid-cols-2">
              {g.items.map((d) => (
                <li key={d.slug}>
                  <Link
                    href={`/docs/${d.slug}`}
                    className="group block h-full bg-ink-950 p-5 transition-colors hover:bg-ink-900"
                  >
                    <p className="font-display text-2xl transition-colors group-hover:text-sand-300">
                      {d.title}
                    </p>
                    <p className="mt-2 text-[15px] leading-relaxed text-dim">{d.summary}</p>
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </>
  );
}
