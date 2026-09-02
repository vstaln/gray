import Link from "next/link";
import { Nav } from "@/components/nav";
import { docsBySection } from "@/lib/docs";

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  const groups = docsBySection();

  return (
    <>
      <Nav />
      <div className="mx-auto flex max-w-[1200px] gap-14 px-6 pb-24 pt-28">
        <aside className="sticky top-24 hidden h-fit w-[210px] shrink-0 lg:block">
          <nav className="space-y-8">
            {groups.map((g) => (
              <div key={g.section}>
                <p className="eyebrow">{g.section}</p>
                <ul className="mt-3 space-y-1.5 border-l border-ink-700 pl-4">
                  {g.items.map((d) => (
                    <li key={d.slug}>
                      <Link
                        href={`/docs/${d.slug}`}
                        className="block text-[15px] text-dim transition-colors hover:text-paper"
                      >
                        {d.title}
                      </Link>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </nav>
        </aside>

        <main className="min-w-0 flex-1">{children}</main>
      </div>
    </>
  );
}
