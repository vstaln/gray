import Marquee from "react-fast-marquee";
import { motion } from "motion/react";

const providers = [
  "OpenRouter",
  "DeepSeek",
  "Groq",
  "OpenAI",
  "ollama",
  "vLLM",
  "LM Studio",
  "xAI / Grok",
  "Codex",
  "Anthropic",
  "Mistral",
  "Together",
];

export function Providers() {
  return (
    <motion.section
      initial={{ opacity: 0 }}
      whileInView={{ opacity: 1 }}
      viewport={{ once: true, amount: 0.6 }}
      transition={{ duration: 1 }}
      className="relative border-y border-ink-800/80 bg-ink-950"
      aria-label="Supported providers"
    >
      <div className="mx-auto flex max-w-7xl items-center gap-6 px-5 sm:px-8">
        <span className="mono shrink-0 py-5 text-[10.5px] uppercase tracking-[0.18em] text-ink-500">
          Any OpenAI-compatible endpoint
        </span>
        <div className="relative min-w-0 flex-1 [mask-image:linear-gradient(90deg,transparent,black_12%,black_88%,transparent)]">
          <Marquee speed={28} gradient={false} pauseOnHover autoFill>
            {providers.map((p) => (
              <span key={p} className="display mx-8 inline-flex items-center gap-8 py-5 text-[15px] font-medium text-ink-400 transition-colors hover:text-ink-50">
                {p}
                <span className="h-1 w-1 rounded-full bg-ink-700" />
              </span>
            ))}
          </Marquee>
        </div>
      </div>
    </motion.section>
  );
}
