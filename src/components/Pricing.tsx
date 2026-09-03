import { ArrowUpRight } from "lucide-react";
import { motion, type Variants } from "motion/react";
import { Magnet } from "./bits/Magnet";
import { RevealImage } from "./bits/RevealImage";
import { SplitText } from "./bits/SplitText";
import { SpotlightCard } from "./bits/SpotlightCard";
import { SectionHead } from "./SectionHead";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";

const tiers = [
  {
    name: "Free",
    price: "$0",
    period: "forever",
    image: { src: "/space/bluemarble.jpg", alt: "Dithered Blue Marble Earth" },
    perks: ["The full binary, MIT", "Every provider, BYOK", "All tools & skills", "Local sessions", "Community support"],
    cta: { label: "Install", href: "#install", live: true },
  },
  {
    name: "Pro",
    price: "$20",
    period: "per month",
    popular: true,
    image: { src: "/space/carina.jpg", alt: "Dithered Carina Nebula cosmic cliffs" },
    perks: ["Hosted gateway", "Telegram · Discord · Slack", "Cloud cron", "Session sync", "Priority builds"],
    cta: { label: "Not yet available", href: "#", live: false },
  },
  {
    name: "Team",
    price: "$100",
    period: "per month",
    image: { src: "/space/andromeda.jpg", alt: "Dithered Andromeda Galaxy" },
    perks: ["Everything in Pro", "Five seats", "Shared skills registry", "Audit log", "SSO"],
    cta: { label: "Not yet available", href: "#", live: false },
  },
];

const grid: Variants = { hidden: {}, show: { transition: { staggerChildren: 0.12 } } };
const card: Variants = {
  hidden: { opacity: 0, y: 30 },
  show: { opacity: 1, y: 0, transition: { duration: 0.9, ease: [0.16, 1, 0.3, 1] } },
};

export function Pricing() {
  return (
    <section id="pricing" className="relative scroll-mt-24 border-t border-ink-800/80 bg-ink-900/30">
      <div className="mx-auto max-w-7xl px-5 py-28 sm:px-8 sm:py-36">
        <SectionHead eyebrow="Pricing" index="§04" align="center">
          <h2 className="display text-[clamp(2rem,4.8vw,3.75rem)] font-semibold leading-[1.02] text-ink-50">
            <SplitText text="Free forever. Paid only for what we host." accent={["forever."]} />
          </h2>
          <p className="prose-tight mx-auto mt-4 max-w-[56ch] text-[16px] leading-relaxed text-ink-300">
            The binary is MIT — every provider, every tool, your own keys. Paid tiers exist only for
            the things we host for you, and are not open for signup yet.
          </p>
        </SectionHead>

        <motion.div
          variants={grid}
          initial="hidden"
          whileInView="show"
          viewport={{ once: true, amount: 0.15 }}
          className="mt-16 grid grid-cols-1 gap-3 md:grid-cols-3"
        >
          {tiers.map((t, i) => (
            <motion.div key={t.name} variants={card} className={t.popular ? "md:-mt-4 md:mb-4" : ""}>
              <SpotlightCard className="flex h-full flex-col">
                <div className="relative h-44 overflow-hidden border-b border-ink-800">
                  <RevealImage src={t.image.src} alt={t.image.alt} from={i === 1 ? "top" : "bottom"} parallax={18} />
                  <div className="absolute inset-0 bg-gradient-to-b from-ink-900/10 via-ink-900/40 to-ink-900" aria-hidden />
                  <div className="absolute left-5 top-5 flex items-center gap-2">
                    <span className="mono text-[11px] tracking-[0.12em] text-accent">#{String(i + 1).padStart(2, "0")}</span>
                    <span className="display text-[15px] font-semibold text-ink-50">{t.name}</span>
                  </div>
                  {t.popular ? (
                    <Badge variant="accent" className="absolute right-5 top-5">
                      Popular
                    </Badge>
                  ) : null}
                </div>

                <div className="relative z-[3] flex flex-1 flex-col p-6">
                  <div className="flex items-baseline gap-2">
                    <span className="display text-[44px] font-semibold leading-none tracking-tight text-ink-50">{t.price}</span>
                    <span className="serif-it text-[17px] text-ink-400">{t.period}</span>
                  </div>
                  <ul className="mt-6 space-y-2.5">
                    {t.perks.map((p) => (
                      <li key={p} className="flex items-baseline gap-3 text-[14px] text-ink-200">
                        <span className="h-px w-3 shrink-0 translate-y-[-3px] bg-ink-600 transition-all duration-500 group-hover:w-4 group-hover:bg-accent" />
                        {p}
                      </li>
                    ))}
                  </ul>
                  <div className="mt-8 flex-1" />
                  {t.cta.live ? (
                    <Magnet strength={0.25} padding={12} className="-m-3 self-start">
                      <a href={t.cta.href}>
                        <Button variant={t.popular ? "accent" : "default"} className="group/btn">
                          {t.cta.label}
                          <ArrowUpRight className="h-3.5 w-3.5 transition-transform duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover/btn:-translate-y-0.5 group-hover/btn:translate-x-0.5" />
                        </Button>
                      </a>
                    </Magnet>
                  ) : (
                    <Button variant="outline" disabled className="self-start">
                      {t.cta.label}
                    </Button>
                  )}
                </div>
              </SpotlightCard>
            </motion.div>
          ))}
        </motion.div>
      </div>
    </section>
  );
}
