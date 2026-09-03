import { motion, useScroll, useTransform } from "motion/react";
import { useRef } from "react";
import { GithubMark } from "./Nav";

export function Footer() {
  const ref = useRef<HTMLElement>(null);
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start end", "end end"] });
  const y = useTransform(scrollYProgress, [0, 1], ["30%", "0%"]);
  const opacity = useTransform(scrollYProgress, [0, 1], [0, 1]);

  return (
    <footer ref={ref} className="relative overflow-hidden border-t border-ink-800/80">
      <div className="mx-auto max-w-7xl px-5 pt-20 sm:px-8">
        <div className="grid grid-cols-1 gap-10 md:grid-cols-12">
          <div className="md:col-span-5">
            <div className="flex items-center gap-2.5">
              <span className="h-1.5 w-1.5 rounded-full bg-accent shadow-[0_0_10px_rgba(217,176,97,0.8)]" />
              <span className="display text-[17px] font-semibold text-ink-50">gray</span>
            </div>
            <p className="prose-tight mt-4 max-w-[40ch] text-[14px] leading-relaxed text-ink-400">
              A minimal, modular agent harness in Rust. It starts at a prompt and gets out of the way.
            </p>
            <p className="serif-it mt-6 text-[17px] text-ink-500">MIT © 2026 vstaln</p>
          </div>

          <div className="grid grid-cols-2 gap-8 md:col-span-7 md:grid-cols-3">
            {[
              { h: "Product", l: [["Features", "#features"], ["The loop", "#loop"], ["Install", "#install"], ["Pricing", "#pricing"]] },
              { h: "Source", l: [["GitHub", "https://github.com/vstaln/gray"], ["Releases", "https://gray.alignment.id/dl"], ["Issues", "https://github.com/vstaln/gray/issues"], ["License", "https://github.com/vstaln/gray/blob/main/LICENSE"]] },
              { h: "Config", l: [["~/.gray/AGENTS.md", "#loop"], ["~/.gray/config.json", "#install"], ["GRAY_LOG=debug", "#loop"], ["/help", "#install"]] },
            ].map((col) => (
              <div key={col.h}>
                <h4 className="mono text-[10.5px] uppercase tracking-[0.18em] text-ink-500">{col.h}</h4>
                <ul className="mt-4 space-y-2.5">
                  {col.l.map(([label, href]) => (
                    <li key={label}>
                      <a
                        href={href}
                        target={href.startsWith("http") ? "_blank" : undefined}
                        rel={href.startsWith("http") ? "noreferrer" : undefined}
                        className="focus-ring group inline-flex items-center gap-1.5 rounded-xs text-[13.5px] text-ink-300 transition-colors hover:text-ink-50"
                      >
                        <span className="h-px w-0 bg-accent transition-all duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:w-3" />
                        <span className={col.h === "Config" ? "mono text-[12.5px]" : ""}>{label}</span>
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </div>

        <div className="mt-14 flex flex-wrap items-center justify-between gap-4 border-t border-ink-800/80 py-6">
          <span className="mono text-[11px] uppercase tracking-[0.16em] text-ink-500">Rust · OpenAI-compatible · JSONL sessions · zero runtime deps</span>
          <a
            href="https://github.com/vstaln/gray"
            target="_blank"
            rel="noreferrer"
            className="focus-ring inline-flex items-center gap-2 rounded-xs text-[12.5px] text-ink-400 transition-colors hover:text-accent"
          >
            <GithubMark className="h-3.5 w-3.5" /> vstaln/gray
          </a>
        </div>
      </div>

      {/* giant outlined wordmark rising from the floor */}
      <motion.div style={{ y, opacity }} className="pointer-events-none relative mx-auto max-w-7xl select-none px-5 sm:px-8" aria-hidden>
        <div className="display outline-text -mb-[0.22em] text-[clamp(7rem,26vw,24rem)] font-semibold leading-none tracking-[-0.06em]">
          gray
        </div>
        <div className="absolute inset-x-0 bottom-0 h-1/2 bg-gradient-to-t from-ink-950 to-transparent" />
      </motion.div>
    </footer>
  );
}
