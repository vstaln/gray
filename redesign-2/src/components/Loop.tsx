import { motion } from "motion/react";
import { SplitText } from "./bits/SplitText";
import { SectionHead } from "./SectionHead";
import { Terminal } from "./Terminal";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "./ui/accordion";

const spec = [
  { k: "Loop", v: "ReAct — streams text, calls tools, feeds results back", d: "The agent loop is a single async task. Every text delta, tool call and tool result is a typed event, so the REPL, the gateway and the cron runner all consume the same stream." },
  { k: "Tools", v: "bash · read · write · edit · grep · find · ls", d: "A small registry with a cancellation token per tool call. Subagents get their own registry so a child can be sandboxed differently from its parent." },
  { k: "Sessions", v: "JSONL on disk, parent-id branching, gray -c to resume", d: "Every event appends immediately. Branch by replying to an earlier turn; the parent-id chain is the tree." },
  { k: "Transport", v: "OpenAI-compatible SSE with typed events and retries", d: "One provider crate talks to every endpoint. Retries with backoff on transient failures, and mid-stream reconnect where the provider supports it." },
  { k: "Context", v: "auto-compacts at window − 16k, /compact to force", d: "Window resolves from flag, env, provider, then a model table. Near the limit it summarizes history into two messages and carries on." },
  { k: "Prompt", v: "identity + guidelines, editable at ~/.gray/AGENTS.md", d: "/agentsmd opens it in $EDITOR. Show, edit, or reset — nothing is hidden behind a config UI." },
];

export function Loop() {
  return (
    <section id="loop" className="relative scroll-mt-24 border-t border-ink-800/80 bg-ink-900/30">
      {/* backdrop: faint dot grid + helix nebula ghost */}
      <div className="dot-grid pointer-events-none absolute inset-0 opacity-40 [mask-image:radial-gradient(ellipse_at_20%_30%,black,transparent_60%)]" aria-hidden />

      <div className="mx-auto grid max-w-7xl grid-cols-1 gap-14 px-5 py-28 sm:px-8 sm:py-36 lg:grid-cols-12 lg:gap-10">
        <div className="lg:col-span-5">
          <SectionHead eyebrow="The loop" index="§02">
            <h2 className="display text-[clamp(2rem,4.8vw,3.75rem)] font-semibold leading-[1.02] text-ink-50">
              <SplitText text="You watch it think." accent={["think."]} />
            </h2>
            <p className="prose-tight mt-4 max-w-[50ch] text-[16px] leading-relaxed text-ink-300">
              Text deltas, tool calls and usage all arrive as typed events over SSE, so the terminal
              shows the turn as it happens. Ctrl-C cancels mid-turn and still persists what reached
              memory.
            </p>
          </SectionHead>

          <motion.div
            initial={{ opacity: 0, y: 20 }}
            whileInView={{ opacity: 1, y: 0 }}
            viewport={{ once: true, amount: 0.3 }}
            transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1], delay: 0.2 }}
            className="mt-10"
          >
            <Accordion type="single" collapsible defaultValue="Loop" className="border-t border-ink-800">
              {spec.map((s) => (
                <AccordionItem key={s.k} value={s.k}>
                  <AccordionTrigger>
                    <span className="grid w-full grid-cols-[88px_1fr] items-baseline gap-4">
                      <span className="mono text-[11px] uppercase tracking-[0.16em] text-accent">{s.k}</span>
                      <span className="text-[14px] text-ink-200">{s.v}</span>
                    </span>
                  </AccordionTrigger>
                  <AccordionContent>
                    <p className="prose-tight ml-[104px] max-w-[46ch] leading-relaxed">{s.d}</p>
                  </AccordionContent>
                </AccordionItem>
              ))}
            </Accordion>
          </motion.div>
        </div>

        <motion.div
          initial={{ opacity: 0, x: 40, rotateY: -6 }}
          whileInView={{ opacity: 1, x: 0, rotateY: 0 }}
          viewport={{ once: true, amount: 0.3 }}
          transition={{ duration: 1.2, ease: [0.16, 1, 0.3, 1] }}
          style={{ perspective: 1200 }}
          className="relative lg:col-span-7 lg:pl-6"
        >
          <div className="sticky top-28">
            <div className="pointer-events-none absolute -inset-8 -z-10 rounded-[24px] bg-accent/[0.05] blur-3xl" aria-hidden />
            <div className="absolute -right-8 -top-8 -z-10 hidden h-56 w-56 lg:block" aria-hidden>
              <img src="/space/helix.jpg" alt="" className="dither h-full w-full rounded-full object-cover opacity-40 [mask-image:radial-gradient(circle,black_40%,transparent_70%)]" />
            </div>
            <Terminal />
            <div className="mt-4 grid grid-cols-3 gap-3">
              {[
                ["text.delta", "streamed"],
                ["tool.call", "typed"],
                ["usage", "priced"],
              ].map(([k, v], i) => (
                <motion.div
                  key={k}
                  initial={{ opacity: 0, y: 12 }}
                  whileInView={{ opacity: 1, y: 0 }}
                  viewport={{ once: true }}
                  transition={{ duration: 0.6, delay: 0.3 + i * 0.1, ease: [0.16, 1, 0.3, 1] }}
                  className="rounded-sm border border-ink-800 bg-ink-950/60 px-3 py-2.5"
                >
                  <span className="mono block text-[11.5px] text-ink-100">{k}</span>
                  <span className="serif-it text-[14px] text-ink-500">{v}</span>
                </motion.div>
              ))}
            </div>
          </div>
        </motion.div>
      </div>
    </section>
  );
}
