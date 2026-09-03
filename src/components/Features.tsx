import { motion, type Variants } from "motion/react";
import { RevealImage } from "./bits/RevealImage";
import { SplitText } from "./bits/SplitText";
import { SpotlightCard } from "./bits/SpotlightCard";
import { TiltedCard } from "./bits/TiltedCard";
import { SectionHead } from "./SectionHead";

type Feature = {
  n: string;
  verb: string;
  title: string;
  body: string;
  image?: { src: string; alt: string; from?: "bottom" | "left" | "right" | "top"; className?: string };
  span: string;
  minH: string;
  tag: string;
};

const features: Feature[] = [
  {
    n: "01",
    verb: "Ship",
    title: "One binary",
    body: "Rust, statically linked, four platforms. No node_modules, no venv, no runtime to install. It starts in milliseconds and it is the whole product.",
    image: { src: "/space/moon.jpg", alt: "Dithered Apollo 16 lunar module on the lunar surface", from: "left" },
    span: "md:col-span-4",
    minH: "min-h-[360px] md:min-h-[440px]",
    tag: "~6 MB · 4 targets",
  },
  {
    n: "02",
    verb: "Connect",
    title: "Any provider",
    body: "OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM, LM Studio — plus OAuth sign-in for xAI/Grok and Codex/ChatGPT accounts. Switch models mid-session.",
    image: { src: "/space/jupiter.jpg", alt: "Dithered Juno image of a Jupiter storm", from: "right" },
    span: "md:col-span-2",
    minH: "min-h-[320px] md:min-h-[440px]",
    tag: "/model · /provider",
  },
  {
    n: "03",
    verb: "Remember",
    title: "Sessions that survive",
    body: "Every turn appends to JSONL on disk with parent-id branching. Ctrl-C mid-turn still persists what reached memory. gray -c reopens the latest.",
    image: { src: "/space/helix.jpg", alt: "Dithered Helix Nebula", from: "top", className: "opacity-90" },
    span: "md:col-span-2",
    minH: "min-h-[280px] md:min-h-[380px]",
    tag: "~/.gray/sessions/*.jsonl",
  },
  {
    n: "04",
    verb: "Delegate",
    title: "Subagents",
    body: "delegate_task spawns isolated children with their own registry and cancellation token, ten concurrent, background-durable through a SQLite queue.",
    image: { src: "/space/saturn.jpg", alt: "Dithered backlit Saturn from Cassini", from: "bottom" },
    span: "md:col-span-4",
    minH: "min-h-[320px] md:min-h-[380px]",
    tag: "10 concurrent",
  },
  {
    n: "05",
    verb: "Live",
    title: "Lives everywhere",
    body: "A gateway daemon puts the same agent on Telegram, Discord and Slack, with per-user sessions and /reset /status /stop. systemd unit included.",
    image: { src: "/space/aurora.jpg", alt: "Dithered aurora over North America from orbit", from: "left" },
    span: "md:col-span-3",
    minH: "min-h-[320px] md:min-h-[400px]",
    tag: "gray gateway",
  },
  {
    n: "06",
    verb: "Schedule",
    title: "Unattended",
    body: "Cron jobs run prompts on a schedule while you are gone — reports, backups, briefings — with inflight guards so a slow run never doubles up.",
    image: { src: "/space/eclipse.jpg", alt: "Dithered total solar eclipse corona", from: "right" },
    span: "md:col-span-3",
    minH: "min-h-[320px] md:min-h-[400px]",
    tag: "gray cron",
  },
];

const grid: Variants = { hidden: {}, show: { transition: { staggerChildren: 0.09 } } };
const cell: Variants = {
  hidden: { opacity: 0, y: 28, scale: 0.985 },
  show: { opacity: 1, y: 0, scale: 1, transition: { duration: 0.8, ease: [0.16, 1, 0.3, 1] } },
};

export function Features() {
  return (
    <section id="features" className="relative mx-auto max-w-7xl scroll-mt-24 px-5 py-28 sm:px-8 sm:py-36">
      <SectionHead eyebrow="What it does" index="§01">
        <h2 className="display text-[clamp(2rem,4.8vw,3.75rem)] font-semibold leading-[1.02] text-ink-50">
          <SplitText text="Six things, done completely." accent={["completely."]} />
        </h2>
        <p className="prose-tight mt-4 max-w-[52ch] text-[16px] leading-relaxed text-ink-300">
          No plugin marketplace, no roadmap promises. Each of these ships in the binary today.
        </p>
      </SectionHead>

      <motion.div
        variants={grid}
        initial="hidden"
        whileInView="show"
        viewport={{ once: true, amount: 0.1 }}
        className="mt-14 grid grid-cols-1 gap-3 md:grid-cols-6"
      >
        {features.map((f, i) => (
          <motion.div key={f.title} variants={cell} className={`${f.span} ${f.minH} flex`}>
            <TiltedCard amount={4} className="w-full">
              <SpotlightCard className="h-full min-h-[inherit]">
                {f.image ? (
                  <>
                    <RevealImage
                      src={f.image.src}
                      alt={f.image.alt}
                      from={f.image.from}
                      delay={i * 0.05}
                      imgClassName={f.image.className}
                    />
                    <div className="absolute inset-0 bg-gradient-to-t from-ink-900 via-ink-900/70 to-ink-900/5" aria-hidden />
                  </>
                ) : (
                  <div
                    className="dot-grid absolute inset-0 opacity-70"
                    aria-hidden
                    style={{
                      maskImage: "radial-gradient(ellipse at 80% 20%, black 20%, transparent 70%)",
                      WebkitMaskImage: "radial-gradient(ellipse at 80% 20%, black 20%, transparent 70%)",
                    }}
                  />
                )}

                {/* Hermes-style numbered eyebrow */}
                <div className="absolute left-6 top-6 flex items-center gap-2 sm:left-7 sm:top-7">
                  <span className="mono text-[11px] tracking-[0.12em] text-accent">#{f.n}</span>
                  <span className="mono text-[11px] uppercase tracking-[0.16em] text-ink-400">{f.verb}</span>
                </div>
                <span className="mono absolute right-6 top-6 text-[10.5px] tracking-[0.1em] text-ink-500 opacity-0 transition-all duration-500 group-hover:opacity-100 group-hover:text-ink-300 sm:right-7 sm:top-7">
                  {f.tag}
                </span>

                <div className="relative z-[3] flex h-full min-h-[inherit] flex-col justify-end p-6 sm:p-7">
                  <h3 className="display text-[26px] font-semibold leading-tight text-ink-50 transition-transform duration-700 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:-translate-y-1 sm:text-[30px]">
                    {f.title}
                  </h3>
                  <p className="prose-tight mt-2.5 max-w-[46ch] text-[14.5px] leading-relaxed text-ink-200 transition-transform duration-700 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:-translate-y-1">
                    {f.body}
                  </p>
                  <span className="mt-4 h-px w-8 origin-left scale-x-0 bg-accent transition-transform duration-700 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:scale-x-100" />
                </div>
              </SpotlightCard>
            </TiltedCard>
          </motion.div>
        ))}
      </motion.div>
    </section>
  );
}
