"use client";

import { motion, useScroll, useSpring } from "motion/react";
import { useRef } from "react";

const specs = [
  {
    name: "Loop",
    text: "ReAct. Streams text, calls tools, feeds results back until the turn is done.",
  },
  {
    name: "Tools",
    text: "A small, complete registry that covers real editing work.",
    chips: ["bash", "read", "write", "edit", "grep", "find", "ls"],
  },
  {
    name: "Sessions",
    text: "JSONL on disk with parent-id branching. gray -c resumes the latest.",
  },
  {
    name: "Transport",
    text: "OpenAI-compatible SSE with typed events and retries.",
  },
  {
    name: "Context",
    text: "Auto-compacts at window minus 16k. /compact forces it.",
  },
  {
    name: "Prompt",
    text: "Identity plus guidelines, editable at ~/.gray/AGENTS.md.",
  },
];

export function Loop() {
  const ref = useRef<HTMLDivElement>(null);
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start 70%", "end 65%"] });
  const scaleY = useSpring(scrollYProgress, { stiffness: 90, damping: 24, mass: 0.4 });

  return (
    <section id="loop" className="relative scroll-mt-24 border-t border-ink-800 bg-ink-900/40">
      <div className="mx-auto grid max-w-7xl grid-cols-1 gap-12 px-5 py-28 sm:px-8 sm:py-36 lg:grid-cols-12 lg:gap-8">
        <div className="lg:col-span-5">
          <div className="lg:sticky lg:top-32">
            <motion.h2
              initial={{ opacity: 0, y: 20 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true, amount: 0.6 }}
              transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1] }}
              className="display text-[clamp(2rem,4.5vw,3.5rem)] font-semibold leading-[1.02] text-ink-50"
            >
              You watch it think.
            </motion.h2>
            <motion.p
              initial={{ opacity: 0, y: 20 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true, amount: 0.6 }}
              transition={{ duration: 0.8, delay: 0.1, ease: [0.16, 1, 0.3, 1] }}
              className="prose-tight mt-5 max-w-[46ch] text-[16px] leading-relaxed text-ink-300"
            >
              Text deltas, tool calls and usage arrive as typed events over SSE, so the terminal
              shows the turn as it happens. Ctrl-C cancels mid-turn and still persists what
              reached memory.
            </motion.p>
          </div>
        </div>

        <div ref={ref} className="relative lg:col-span-6 lg:col-start-7">
          {/* Track and the scroll-driven line */}
          <div className="absolute bottom-3 left-[7px] top-3 w-px bg-ink-700" aria-hidden />
          <motion.div
            style={{ scaleY, transformOrigin: "top" }}
            className="absolute bottom-3 left-[7px] top-3 w-px bg-accent"
            aria-hidden
          />

          <ol className="flex flex-col gap-10 sm:gap-12">
            {specs.map((s, i) => (
              <motion.li
                key={s.name}
                initial={{ opacity: 0, x: 16 }}
                whileInView={{ opacity: 1, x: 0 }}
                viewport={{ once: true, amount: 0.7 }}
                transition={{ duration: 0.7, delay: i * 0.04, ease: [0.16, 1, 0.3, 1] }}
                className="relative pl-10"
              >
                <span
                  className="absolute left-0 top-[9px] h-[15px] w-[15px] rounded-full border border-ink-600 bg-ink-950"
                  aria-hidden
                >
                  <motion.span
                    initial={{ scale: 0 }}
                    whileInView={{ scale: 1 }}
                    viewport={{ once: true, amount: 1 }}
                    transition={{ duration: 0.5, delay: 0.25, ease: [0.16, 1, 0.3, 1] }}
                    className="absolute inset-[3px] rounded-full bg-accent"
                  />
                </span>
                <h3 className="mono text-[13px] uppercase tracking-[0.16em] text-ink-50">{s.name}</h3>
                <p className="prose-tight mt-2 max-w-[48ch] text-[16px] leading-relaxed text-ink-300">
                  {s.text}
                </p>
                {s.chips ? (
                  <div className="mt-4 flex flex-wrap gap-2">
                    {s.chips.map((c) => (
                      <span
                        key={c}
                        className="mono rounded-xs border border-ink-700 bg-ink-900 px-2.5 py-1 text-[12.5px] text-ink-200"
                      >
                        {c}
                      </span>
                    ))}
                  </div>
                ) : null}
              </motion.li>
            ))}
          </ol>
        </div>
      </div>
    </section>
  );
}
