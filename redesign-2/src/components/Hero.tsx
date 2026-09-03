import { ArrowDown } from "lucide-react";
import { motion, useReducedMotion, useScroll, useTransform, type Variants } from "motion/react";
import { useRef } from "react";
import { Particles } from "./bits/Particles";
import { ShinyText } from "./bits/ShinyText";
import { SplitText } from "./bits/SplitText";
import { InstallCommand } from "./InstallCommand";
import { Terminal } from "./Terminal";
import { Badge } from "./ui/badge";

const stagger: Variants = {
  hidden: {},
  show: { transition: { staggerChildren: 0.1, delayChildren: 0.9 } },
};
const rise: Variants = {
  hidden: { opacity: 0, y: 24, filter: "blur(8px)" },
  show: { opacity: 1, y: 0, filter: "blur(0px)", transition: { duration: 0.9, ease: [0.16, 1, 0.3, 1] } },
};

export function Hero() {
  const ref = useRef<HTMLElement>(null);
  const reduce = useReducedMotion();
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start start", "end start"] });
  const imgY = useTransform(scrollYProgress, [0, 1], ["0%", reduce ? "0%" : "22%"]);
  const imgScale = useTransform(scrollYProgress, [0, 1], [1, reduce ? 1 : 1.12]);
  const imgOpacity = useTransform(scrollYProgress, [0, 0.8], [1, 0.15]);
  const textY = useTransform(scrollYProgress, [0, 1], ["0%", reduce ? "0%" : "-12%"]);
  const termY = useTransform(scrollYProgress, [0, 1], ["0%", reduce ? "0%" : "10%"]);

  return (
    <section ref={ref} id="top" className="relative isolate min-h-[100svh] overflow-hidden">
      {/* starfield */}
      <div className="absolute inset-0 -z-20">
        <Particles count={120} />
      </div>

      {/* Dithered Earth limb, bleeding off the right edge. */}
      <motion.div
        style={{ y: imgY, scale: imgScale, opacity: imgOpacity }}
        initial={{ opacity: 0, scale: 1.08, filter: "blur(14px)" }}
        animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
        transition={{ duration: 2.2, ease: [0.16, 1, 0.3, 1], delay: 0.2 }}
        className="pointer-events-none absolute -right-[28%] top-[2%] -z-10 aspect-square w-[110vw] max-w-[1100px] md:-right-[10%] md:top-[-4%] md:w-[68vw]"
        aria-hidden
      >
        <div className="relative h-full w-full animate-drift opacity-50 md:opacity-100">
          <img
            src="/space/earth.jpg"
            alt=""
            className="dither h-full w-full object-cover opacity-70"
            style={{
              maskImage: "radial-gradient(ellipse at 60% 45%, black 40%, transparent 72%)",
              WebkitMaskImage: "radial-gradient(ellipse at 60% 45%, black 40%, transparent 72%)",
            }}
          />
        </div>
        {/* ochre atmospheric rim */}
        <div
          className="absolute inset-0 opacity-70 mix-blend-screen"
          style={{
            background:
              "radial-gradient(ellipse at 58% 46%, transparent 38%, rgba(217,176,97,0.14) 46%, transparent 56%)",
          }}
        />
      </motion.div>

      {/* vignette to floor */}
      <div className="pointer-events-none absolute inset-x-0 bottom-0 -z-10 h-64 bg-gradient-to-b from-transparent to-ink-950" />

      <div className="mx-auto grid max-w-7xl grid-cols-1 gap-12 px-5 pb-24 pt-32 sm:px-8 sm:pt-40 lg:grid-cols-12 lg:gap-8 lg:pb-32">
        <motion.div style={{ y: textY }} className="lg:col-span-7">
          <motion.div variants={stagger} initial="hidden" animate="show">
            <motion.div variants={rise} className="flex flex-wrap items-center gap-2">
              <Badge variant="accent">
                <span className="h-1 w-1 rounded-full bg-accent" /> v0 · open source
              </Badge>
              <span className="mono text-[11px] uppercase tracking-[0.16em] text-ink-500">Rust · OpenAI-compatible · MIT</span>
            </motion.div>
          </motion.div>

          <h1 className="display mt-7 text-[clamp(2.75rem,7.4vw,6.25rem)] font-semibold leading-[0.96] text-ink-50">
            <SplitText text="The agent that" delay={0.35} as="span" className="block" />
            <SplitText text="fits in one binary." delay={0.7} as="span" className="block" accent={["one"]} />
          </h1>

          <motion.div variants={stagger} initial="hidden" animate="show">
            <motion.p variants={rise} className="prose-tight mt-7 max-w-[54ch] text-[17px] leading-relaxed text-ink-300 sm:text-[18px]">
              gray runs tools, edits code, and streams from any model provider. No runtime, no
              node_modules, no dashboard. It starts at a prompt and gets out of the way.
            </motion.p>

            <motion.div variants={rise} className="mt-9 max-w-xl">
              <InstallCommand size="lg" command="curl -fsSL https://gray.alignment.id/install.sh | sh" />
              <p className="mono mt-3 text-[11px] uppercase tracking-[0.14em] text-ink-500">
                macOS · Linux · WSL — <a href="#install" className="text-ink-400 underline-offset-4 hover:text-accent hover:underline">Windows &amp; source builds</a>
              </p>
            </motion.div>

            <motion.div variants={rise} className="mt-10 flex items-center gap-6">
              <a href="#features" className="focus-ring group inline-flex items-center gap-2 rounded-xs text-[13px] text-ink-300 transition-colors hover:text-ink-50">
                <span className="grid h-8 w-8 place-items-center rounded-full border border-ink-700 transition-colors group-hover:border-accent">
                  <ArrowDown className="h-3.5 w-3.5 transition-transform duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:translate-y-0.5" />
                </span>
                See what it does
              </a>
              <ShinyText text="Ships in ~6 MB. Starts in milliseconds." className="hidden text-[13px] sm:inline" />
            </motion.div>
          </motion.div>
        </motion.div>

        <motion.div
          style={{ y: termY }}
          initial={{ opacity: 0, y: 40, rotateX: 8 }}
          animate={{ opacity: 1, y: 0, rotateX: 0 }}
          transition={{ duration: 1.3, ease: [0.16, 1, 0.3, 1], delay: 1.2 }}
          className="relative lg:col-span-5 lg:self-end"
        >
          <div className="pointer-events-none absolute -inset-6 -z-10 rounded-[20px] bg-accent/[0.04] blur-3xl" />
          <Terminal />
          <div className="mt-3 flex items-center justify-between">
            <span className="mono text-[10.5px] uppercase tracking-[0.16em] text-ink-500">Feature</span>
            <span className="serif-it text-[15px] text-ink-400">preview · a real turn, event by event</span>
          </div>
        </motion.div>
      </div>
    </section>
  );
}
