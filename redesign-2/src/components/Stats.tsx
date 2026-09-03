import { motion } from "motion/react";
import { CountUp } from "./bits/CountUp";

const stats = [
  { to: 1, suffix: "", label: "binary", note: "the whole product" },
  { to: 4, suffix: "", label: "platforms", note: "macOS · Linux · WSL · Windows" },
  { to: 10, suffix: "", label: "concurrent subagents", note: "SQLite-durable queue" },
  { to: 16, suffix: "k", label: "token reserve", note: "before auto-compact" },
  { to: 0, suffix: "", label: "runtime deps", note: "no node, no venv" },
];

export function Stats() {
  return (
    <section className="relative border-y border-ink-800/80" aria-label="By the numbers">
      <div className="mx-auto grid max-w-7xl grid-cols-2 divide-x divide-ink-800/80 px-5 sm:px-8 md:grid-cols-5">
        {stats.map((s, i) => (
          <motion.div
            key={s.label}
            initial={{ opacity: 0, y: 16 }}
            whileInView={{ opacity: 1, y: 0 }}
            viewport={{ once: true, amount: 0.6 }}
            transition={{ duration: 0.7, delay: i * 0.08, ease: [0.16, 1, 0.3, 1] }}
            className="group px-5 py-10 first:pl-0 last:pr-0 md:py-14"
          >
            <div className="display text-[clamp(2.4rem,5vw,4rem)] font-semibold leading-none text-ink-50">
              <CountUp to={s.to} suffix={s.suffix} duration={1.4 + i * 0.15} className="font-[inherit] tracking-tight" />
            </div>
            <div className="mt-3 text-[13.5px] text-ink-200">{s.label}</div>
            <div className="serif-it mt-0.5 text-[14px] text-ink-500 transition-colors group-hover:text-accent">{s.note}</div>
          </motion.div>
        ))}
      </div>
    </section>
  );
}
