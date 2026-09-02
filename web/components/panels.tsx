import Image from "next/image";
import { panels } from "@/lib/site";
import { cn } from "@/lib/cn";

export function Panels() {
  return (
    <section id="panels" className="relative border-t border-ink-700">
      <div className="mx-auto max-w-[1200px] px-6 py-24">
        <p className="eyebrow">What it does</p>
        <h2 className="display mt-4 max-w-[18ch] text-[clamp(2.5rem,5vw,4.5rem)]">
          Six things, done completely.
        </h2>
      </div>

      <div className="mx-auto grid max-w-[1200px] grid-cols-1 border-t border-ink-700 md:grid-cols-2 lg:grid-cols-3">
        {panels.map((p, i) => (
          <article
            key={p.n}
            className={cn(
              "group relative border-b border-ink-700 px-6 py-10 transition-colors hover:bg-ink-900/60",
              "md:border-r",
              i % 2 === 1 && "md:border-r-0",
              "lg:border-r",
              (i + 1) % 3 === 0 && "lg:border-r-0",
            )}
          >
            <div className="flex items-baseline gap-3">
              <span className="font-mono text-[11px] text-sand-400">#{p.n}</span>
              <span className="eyebrow">{p.kicker}</span>
            </div>

            <h3 className="display mt-5 text-[2rem]">{p.title}</h3>

            <div className="relative mt-6 aspect-[16/9] overflow-hidden border border-ink-700 bg-ink-950">
              <Image
                src={p.image}
                alt={p.alt}
                fill
                sizes="(max-width: 768px) 100vw, (max-width: 1200px) 50vw, 33vw"
                className="object-cover opacity-70 transition-all duration-700 ease-[var(--ease-out-expo)] group-hover:scale-105 group-hover:opacity-95"
              />
            </div>

            <p className="mt-6 text-[15px] leading-relaxed text-dim">{p.body}</p>
          </article>
        ))}
      </div>
    </section>
  );
}
