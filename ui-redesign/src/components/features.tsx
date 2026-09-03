"use client";

import { motion, type Variants } from "motion/react";
import Image from "next/image";
import type { MouseEvent, ReactNode } from "react";

type Feature = {
  title: string;
  body: string;
  image?: { src: string; alt: string; position?: string };
  span: string;
  minH: string;
};

const features: Feature[] = [
  {
    title: "One binary",
    body: "Rust, statically linked, four platforms. Nothing to install around it. It starts in milliseconds and it is the whole product.",
    image: { src: "/space/moon.png", alt: "Dithered Apollo lunar module on the lunar surface", position: "object-center" },
    span: "md:col-span-4",
    minH: "min-h-[360px] md:min-h-[420px]",
  },
  {
    title: "Any provider",
    body: "OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM, LM Studio. OAuth sign-in for xAI and Codex accounts. Switch models mid-session.",
    image: { src: "/space/jupiter.png", alt: "Dithered storm on Jupiter photographed by Juno", position: "object-center" },
    span: "md:col-span-2",
    minH: "min-h-[320px] md:min-h-[420px]",
  },
  {
    title: "Sessions that survive",
    body: "Every turn appends to JSONL on disk with parent-id branching. Ctrl-C mid-turn still persists what reached memory. gray -c reopens the latest.",
    span: "md:col-span-2",
    minH: "min-h-[260px] md:min-h-[360px]",
  },
  {
    title: "Subagents",
    body: "delegate_task spawns isolated children with their own registry and cancellation token. Ten concurrent, background-durable through a SQLite queue.",
    image: { src: "/space/saturn.png", alt: "Dithered backlit Saturn photographed by Cassini", position: "object-center" },
    span: "md:col-span-4",
    minH: "min-h-[320px] md:min-h-[360px]",
  },
  {
    title: "Lives everywhere",
    body: "A gateway daemon puts the same agent on Telegram, Discord and Slack with per-user sessions and /reset /status /stop. systemd unit included.",
    image: { src: "/space/aurora.png", alt: "Dithered aurora over Earth seen from orbit", position: "object-center" },
    span: "md:col-span-3",
    minH: "min-h-[320px] md:min-h-[380px]",
  },
  {
    title: "Unattended",
    body: "Cron jobs run prompts on a schedule while you are gone. Reports, backups, briefings, with inflight guards so a slow run never doubles up.",
    image: { src: "/space/eclipse.png", alt: "Dithered total solar eclipse corona", position: "object-center" },
    span: "md:col-span-3",
    minH: "min-h-[320px] md:min-h-[380px]",
  },
];

const grid: Variants = {
  hidden: {},
  show: { transition: { staggerChildren: 0.09 } },
};

const cell: Variants = {
  hidden: { opacity: 0, y: 28, scale: 0.985 },
  show: { opacity: 1, y: 0, scale: 1, transition: { duration: 0.8, ease: [0.16, 1, 0.3, 1] } },
};

function Cell({ children, className }: { children: ReactNode; className: string }) {
  const onMove = (e: MouseEvent<HTMLElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    e.currentTarget.style.setProperty("--mx", `${e.clientX - r.left}px`);
    e.currentTarget.style.setProperty("--my", `${e.clientY - r.top}px`);
  };
  return (
    <motion.article
      variants={cell}
      onMouseMove={onMove}
      className={`spotlight group relative overflow-hidden rounded-md bg-ink-900 ${className}`}
    >
      {children}
    </motion.article>
  );
}

export function Features() {
  return (
    <section id="features" className="mx-auto max-w-7xl scroll-mt-24 px-5 py-28 sm:px-8 sm:py-36">
      <motion.div
        initial={{ opacity: 0, y: 20 }}
        whileInView={{ opacity: 1, y: 0 }}
        viewport={{ once: true, amount: 0.6 }}
        transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1] }}
        className="max-w-2xl"
      >
        <h2 className="display text-[clamp(2rem,4.5vw,3.5rem)] font-semibold leading-[1.02] text-ink-50">
          Six things, done completely.
        </h2>
        <p className="prose-tight mt-4 max-w-[52ch] text-[16px] leading-relaxed text-ink-300">
          No plugin marketplace, no roadmap promises. Each of these ships in the binary today.
        </p>
      </motion.div>

      <motion.div
        variants={grid}
        initial="hidden"
        whileInView="show"
        viewport={{ once: true, amount: 0.12 }}
        className="mt-14 grid grid-cols-1 gap-3 md:grid-cols-6"
      >
        {features.map((f) => (
          <Cell key={f.title} className={`${f.span} ${f.minH}`}>
            {f.image ? (
              <div className="absolute inset-0" aria-hidden>
                <Image
                  src={f.image.src}
                  alt={f.image.alt}
                  fill
                  sizes="(min-width: 768px) 50vw, 100vw"
                  className={`dither ${f.image.position ?? ""} object-cover opacity-55 transition-transform duration-[1400ms] ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:scale-[1.04]`}
                />
                <div className="absolute inset-0 bg-gradient-to-t from-ink-900 via-ink-900/70 to-ink-900/10" />
              </div>
            ) : (
              <div
                className="absolute inset-0 opacity-70"
                aria-hidden
                style={{
                  backgroundImage:
                    "radial-gradient(circle at 1px 1px, rgba(255,255,255,0.09) 1px, transparent 0)",
                  backgroundSize: "14px 14px",
                  maskImage: "radial-gradient(ellipse at 80% 20%, black 20%, transparent 70%)",
                  WebkitMaskImage: "radial-gradient(ellipse at 80% 20%, black 20%, transparent 70%)",
                }}
              />
            )}

            <div className="relative flex h-full flex-col justify-end p-6 sm:p-7">
              <h3 className="display text-[26px] font-semibold leading-tight text-ink-50 sm:text-[28px]">
                {f.title}
              </h3>
              <p className="prose-tight mt-2.5 max-w-[46ch] text-[14.5px] leading-relaxed text-ink-200">
                {f.body}
              </p>
            </div>
          </Cell>
        ))}
      </motion.div>
    </section>
  );
}
