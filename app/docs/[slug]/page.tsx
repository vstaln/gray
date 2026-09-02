import Link from "next/link";
import { notFound } from "next/navigation";
import type { Metadata } from "next";
import { CopyCommand } from "@/components/copy-command";
import { docBySlug, docs } from "@/lib/docs";

export function generateStaticParams() {
  return docs.map((d) => ({ slug: d.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}): Promise<Metadata> {
  const { slug } = await params;
  const doc = docBySlug(slug);
  if (!doc) return {};
  return { title: doc.title, description: doc.summary };
}

export default async function DocPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const doc = docBySlug(slug);
  if (!doc) notFound();

  const idx = docs.findIndex((d) => d.slug === slug);
  const prev = docs[idx - 1];
  const next = docs[idx + 1];

  return (
    <article className="max-w-[70ch]">
      <p className="eyebrow">{doc.section}</p>
      <h1 className="display mt-4 text-[clamp(2.25rem,4.5vw,3.5rem)]">{doc.title}</h1>
      <p className="mt-5 text-lg text-dim">{doc.summary}</p>

      {doc.source ? (
        <p className="mt-5 font-mono text-[11px] text-dimmer">
          Source: <span className="text-sand-400">{doc.source}</span>
        </p>
      ) : null}

      <div className="mt-12 space-y-6 border-t border-ink-700 pt-10">
        {doc.blocks.map((b, i) => {
          switch (b.t) {
            case "h":
              return (
                <h2 key={i} className="display pt-6 text-[1.75rem]">
                  {b.text}
                </h2>
              );
            case "p":
              return (
                <p key={i} className="leading-relaxed text-dim">
                  {b.text}
                </p>
              );
            case "code":
              return b.text.includes("\n") ? (
                <pre
                  key={i}
                  className="overflow-x-auto border border-ink-700 bg-ink-900 p-5 font-mono text-[13px] leading-[1.9] text-paper"
                >
                  {b.text}
                </pre>
              ) : (
                <CopyCommand key={i} command={b.text} />
              );
            case "table":
              return (
                <table key={i} className="w-full border-collapse text-left">
                  <thead>
                    <tr className="border-b border-ink-700">
                      <th className="eyebrow py-2 pr-6 font-normal">{b.head[0]}</th>
                      <th className="eyebrow py-2 font-normal">{b.head[1]}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {b.rows.map(([k, v]) => (
                      <tr key={k} className="border-b border-ink-800">
                        <td className="py-2.5 pr-6 align-top font-mono text-[12px] text-sand-400">
                          {k}
                        </td>
                        <td className="py-2.5 align-top text-[15px] text-dim">{v}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              );
            case "note":
              return (
                <aside
                  key={i}
                  className="border-l-2 border-sand-400/50 bg-ink-900/60 px-5 py-4 text-[15px] text-dim"
                >
                  {b.text}
                </aside>
              );
          }
        })}
      </div>

      <nav className="mt-20 grid gap-px border-t border-ink-700 pt-8 sm:grid-cols-2">
        {prev ? (
          <Link href={`/docs/${prev.slug}`} className="group py-3">
            <p className="eyebrow">Previous</p>
            <p className="mt-1 font-display text-xl transition-colors group-hover:text-sand-300">
              {prev.title}
            </p>
          </Link>
        ) : (
          <span />
        )}
        {next ? (
          <Link href={`/docs/${next.slug}`} className="group py-3 sm:text-right">
            <p className="eyebrow">Next</p>
            <p className="mt-1 font-display text-xl transition-colors group-hover:text-sand-300">
              {next.title}
            </p>
          </Link>
        ) : null}
      </nav>
    </article>
  );
}
