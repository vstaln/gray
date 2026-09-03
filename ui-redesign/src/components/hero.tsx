"use client";

import { motion, useReducedMotion, useScroll, useTransform, type Variants } from "motion/react";
import Image from "next/image";
import { useRef } from "react";
import { InstallCommand } from "./install-command";

const stagger: Variants = {
  hidden: {},
  show: { transition: { staggerChildren: 0.1, delayChildren: 0.15 } },
};

const rise: Variants = {
  hidden: { opacity: 0, y: 24, filter: "blur(8px)" },
  show: {
    opacity: 1,
    y: 0,
    filter: "blur(0px)",
    transition: { duration: 0.9, ease: [0.16, 1, 0.3, 1] },
  },
};

export function Hero() {
  const ref = useRef<HTMLElement>(null);
  const reduce = useReducedMotion();
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start start", "end start"] });
  const imgY = useTransform(scrollYProgress, [0, 1], ["0%", reduce ? "0%" : "18%"]);
  const imgOpacity = useTransform(scrollYProgress, [0, 0.8], [1, 0.25]);

  return (
    <section ref={ref} className="relative overflow-hidden">
      {/* Dithered Earth limb, bleeding off the right edge. */}
      <motion.div
        style={{ y: imgY, opacity: imgOpacity }}
        className="pointer-events-none absolute inset-y-0 right-0 w-full lg:w-[62%]"
        aria-hidden
      >
        <motion.div
          initial={{ opacity: 0, scale: 1.04 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 1.6, ease: [0.16, 1, 0.3, 1] }}
          className="relative h-full w-full"
        >
          <Image
            src="/space/hero.png"
            alt=""
            fill
            priority
            sizes="(min-width: 1024px) 62vw, 100vw"
            className="dither object-cover object-left-top opacity-70 lg:opacity-90"
          />
          <div className="absolute inset-0 bg-gradient-to-r from-ink-950 via-ink-950/70 to-transparent lg:via-ink-950/20" />
          <div className="absolute inset-x-0 bottom-0 h-40 bg-gradient-to-t from-ink-950 to-transparent" />
        </motion.div>
      </motion.div>

      <motion.div
        variants={stagger}
        initial="hidden"
        animate="show"
        className="relative mx-auto flex min-h-[100dvh] max-w-7xl flex-col justify-center px-5 pb-20 pt-24 sm:px-8"
      >
        <motion.p variants={rise} className="mono text-[12px] uppercase tracking-[0.18em] text-ink-300">
          Rust, OpenAI-compatible, MIT
        </motion.p>

        <motion.h1
          variants={rise}
          className="display mt-5 max-w-[12ch] text-[clamp(2.75rem,8vw,6.5rem)] font-semibold leading-[0.98] text-ink-50"
        >
          The agent that fits in one binary.
        </motion.h1>

        <motion.p variants={rise} className="prose-tight mt-6 max-w-[42ch] text-[17px] leading-[1.55] text-ink-300 sm:text-lg">
          gray runs tools, edits code, and streams from any model provider. No runtime, no
          node_modules, no dashboard.
        </motion.p>

        <motion.div variants={rise} className="mt-9 flex flex-col gap-3 sm:flex-row sm:items-center">
          <InstallCommand
            size="lg"
            command="curl -fsSL https://gray.alignment.id/install.sh | sh"
            className="w-full max-w-xl"
          />
          <a
            href="#features"
            className="focus-ring inline-flex h-[50px] shrink-0 items-center justify-center rounded-sm px-4 text-[15px] text-ink-200 transition-colors duration-200 hover:text-ink-50"
          >
            See what it does
          </a>
        </motion.div>
      </motion.div>
    </section>
  );
}
