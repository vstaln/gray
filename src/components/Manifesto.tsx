import { motion, useScroll, useTransform, type MotionValue } from "motion/react";
import { useRef } from "react";
import { cn } from "../utils/cn";

const text =
  "No runtime. No dashboard. No node_modules. A prompt, a loop, and an agent that gets out of the way — small enough to read, complete enough to trust.";

const accentWords = new Set(["prompt,", "loop,", "read,", "trust."]);

function Word({ children, range, progress, accent }: { children: string; range: [number, number]; progress: MotionValue<number>; accent: boolean }) {
  const opacity = useTransform(progress, range, [0.12, 1]);
  const y = useTransform(progress, range, [6, 0]);
  return (
    <motion.span style={{ opacity, y }} className={cn("inline-block will-change-[opacity,transform]", accent && "serif-it text-accent")}>
      {children}
    </motion.span>
  );
}

/**
 * ReactBits-style ScrollReveal: each word brightens as the section scrolls through.
 */
export function Manifesto() {
  const ref = useRef<HTMLElement>(null);
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start 0.85", "end 0.45"] });
  const words = text.split(" ");
  const lineX = useTransform(scrollYProgress, [0, 1], ["-100%", "0%"]);

  return (
    <section ref={ref} className="relative border-t border-ink-800/80 bg-ink-950">
      <div className="pointer-events-none absolute inset-x-0 top-0 h-px overflow-hidden" aria-hidden>
        <motion.div style={{ x: lineX }} className="h-full w-full bg-gradient-to-r from-transparent via-accent to-transparent" />
      </div>
      <div className="mx-auto max-w-7xl px-5 py-32 sm:px-8 sm:py-44">
        <div className="mb-10 flex items-center gap-4">
          <span className="mono text-[11px] tracking-[0.14em] text-accent">§—</span>
          <span className="h-px w-12 bg-ink-600" />
          <span className="mono text-[11px] uppercase tracking-[0.18em] text-ink-400">Why it is small</span>
        </div>
        <p className="display max-w-5xl text-[clamp(1.75rem,4.6vw,3.9rem)] font-medium leading-[1.12] tracking-[-0.03em] text-ink-50">
          {words.map((w, i) => {
            const start = i / words.length;
            const end = Math.min(1, start + 1.6 / words.length);
            return (
              <span key={i}>
                <Word range={[start, end]} progress={scrollYProgress} accent={accentWords.has(w)}>
                  {w}
                </Word>{" "}
              </span>
            );
          })}
        </p>
      </div>
    </section>
  );
}
