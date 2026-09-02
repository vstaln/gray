import Image from "next/image";
import Link from "next/link";
import { Backdrop } from "@/components/backdrop";
import { tiers } from "@/lib/site";
import { cn } from "@/lib/cn";

export function Pricing() {
  return (
    <section id="pricing" className="relative isolate overflow-hidden border-t border-ink-700">
      <Backdrop src="/space/mwcore-plate.jpg" opacity={0.34} blur={3} fade="both" />

      <div className="relative mx-auto max-w-[1200px] px-6 py-24">
        <p className="eyebrow">Pricing</p>
        <h2 className="display mt-4 text-[clamp(2.5rem,5vw,4.5rem)]">Manage Subscription</h2>
        <p className="mt-5 max-w-[62ch] text-dim">
          The binary is MIT and free forever — every provider, every tool, your own keys. Paid tiers
          exist only for the things we host for you.
        </p>

        <div className="mt-14 grid gap-px border border-ink-700 bg-ink-700 md:grid-cols-3">
          {tiers.map((t) => (
            <article
              key={t.id}
              className={cn(
                "flex flex-col p-7",
                t.featured ? "bg-sand-400 text-ink-950" : "bg-ink-900",
              )}
            >
              <div className="flex items-start justify-between">
                <span
                  className={cn(
                    "eyebrow border px-2 py-1",
                    t.featured
                      ? "border-ink-950/40 text-ink-950"
                      : "border-ink-600 text-paper",
                  )}
                >
                  {t.name}
                </span>
                {t.featured ? (
                  <span className="eyebrow border border-ink-950/40 px-2 py-1 text-ink-950">
                    Popular
                  </span>
                ) : null}
              </div>

              <p
                className={cn(
                  "display mt-8 text-[4rem] leading-none",
                  t.featured ? "text-ink-950" : "text-paper",
                )}
              >
                {t.price}
              </p>
              <p
                className={cn(
                  "eyebrow mt-3",
                  t.featured ? "text-ink-950/70" : "text-dim",
                )}
              >
                {t.period}
              </p>

              <div
                className={cn(
                  "relative mt-7 aspect-[16/9] overflow-hidden border",
                  t.featured ? "border-ink-950/25" : "border-ink-700",
                )}
              >
                <Image
                  src={t.image}
                  alt={t.alt}
                  fill
                  sizes="(max-width: 768px) 100vw, 33vw"
                  className={cn(
                    "object-cover",
                    t.featured ? "opacity-40 mix-blend-multiply" : "opacity-75",
                  )}
                />
              </div>

              <ul className="mt-7 flex-1 space-y-2.5">
                {t.features.map((f) => (
                  <li
                    key={f}
                    className={cn(
                      "font-mono text-[11px] uppercase leading-relaxed tracking-[0.08em]",
                      t.featured ? "text-ink-950/80" : "text-dim",
                    )}
                  >
                    <span className={cn("mr-2", t.featured ? "text-ink-950/50" : "text-ink-500")}>
                      ·
                    </span>
                    {f}
                  </li>
                ))}
              </ul>

              <Link
                href={t.href}
                className={cn(
                  "eyebrow mt-9 border px-4 py-3 text-center transition-colors",
                  t.featured
                    ? "border-ink-950 bg-ink-950 text-sand-300 hover:bg-transparent hover:text-ink-950"
                    : "border-ink-600 text-paper hover:border-sand-400 hover:bg-sand-400 hover:text-ink-950",
                )}
              >
                {t.cta}
              </Link>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
