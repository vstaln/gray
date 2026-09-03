import { motion, useMotionValueEvent, useScroll, useSpring } from "motion/react";
import { useState } from "react";
import { cn } from "../utils/cn";
import { Button } from "./ui/button";

const links = [
  { href: "#features", label: "Features" },
  { href: "#loop", label: "The loop" },
  { href: "#install", label: "Install" },
  { href: "#pricing", label: "Pricing" },
];

export function GithubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden>
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.1.79-.25.79-.56v-2.17c-3.2.7-3.87-1.37-3.87-1.37-.52-1.33-1.28-1.69-1.28-1.69-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.18 1.76 1.18 1.03 1.76 2.7 1.25 3.35.96.1-.75.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.68 0-1.26.45-2.28 1.18-3.09-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.78 0c2.2-1.49 3.17-1.18 3.17-1.18.62 1.58.23 2.75.11 3.04.74.81 1.18 1.83 1.18 3.09 0 4.41-2.69 5.38-5.26 5.67.41.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56A11.5 11.5 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z" />
    </svg>
  );
}

export function Nav() {
  const { scrollY, scrollYProgress } = useScroll();
  const progress = useSpring(scrollYProgress, { stiffness: 120, damping: 24, mass: 0.3 });
  const [scrolled, setScrolled] = useState(false);
  useMotionValueEvent(scrollY, "change", (v) => setScrolled(v > 24));

  return (
    <motion.header
      initial={{ y: -24, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      transition={{ duration: 0.9, ease: [0.16, 1, 0.3, 1], delay: 0.2 }}
      className="fixed inset-x-0 top-0 z-40"
    >
      <div
        className={cn(
          "mx-auto flex h-16 max-w-7xl items-center justify-between px-5 transition-[background-color,backdrop-filter,border-color] duration-500 sm:px-8",
        )}
      >
        <a href="#top" className="focus-ring group flex items-center gap-2.5 rounded-xs">
          <span className="relative grid h-6 w-6 place-items-center">
            <span className="absolute inset-0 rounded-xs border border-ink-600 transition-colors group-hover:border-accent" />
            <span className="h-1.5 w-1.5 rounded-full bg-accent shadow-[0_0_10px_rgba(217,176,97,0.8)]" />
          </span>
          <span className="display text-[17px] font-semibold tracking-tight text-ink-50">gray</span>
          <span className="serif-it hidden text-[15px] text-ink-500 sm:inline">/ agent harness</span>
        </a>

        <nav className="hidden items-center gap-1 md:flex" aria-label="Primary">
          {links.map((l) => (
            <a
              key={l.href}
              href={l.href}
              className="focus-ring group relative rounded-xs px-3 py-1.5 text-[13px] text-ink-300 transition-colors hover:text-ink-50"
            >
              {l.label}
              <span className="absolute inset-x-3 -bottom-0.5 h-px origin-left scale-x-0 bg-accent transition-transform duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:scale-x-100" />
            </a>
          ))}
        </nav>

        <div className="flex items-center gap-2">
          <a
            href="https://github.com/vstaln/gray"
            target="_blank"
            rel="noreferrer"
            className="focus-ring mono inline-flex h-9 items-center gap-2 rounded-sm border border-ink-800 bg-ink-900/50 px-3 text-[12px] text-ink-300 backdrop-blur transition-colors hover:border-ink-600 hover:text-ink-50"
          >
            <GithubMark className="h-3.5 w-3.5" />
            <span className="hidden sm:inline">vstaln/gray</span>
          </a>
          <a href="#install">
            <Button size="sm" variant="default" className="h-9">
              Install
            </Button>
          </a>
        </div>
      </div>

      {/* glass background fades in on scroll */}
      <div
        aria-hidden
        className={cn(
          "pointer-events-none absolute inset-0 -z-10 border-b border-transparent bg-ink-950/0 backdrop-blur-0 transition-all duration-500",
          scrolled && "border-ink-800/80 bg-ink-950/70 backdrop-blur-xl",
        )}
      />
      {/* scroll progress */}
      <motion.div
        aria-hidden
        style={{ scaleX: progress }}
        className="absolute inset-x-0 bottom-0 h-px origin-left bg-gradient-to-r from-accent via-accent-strong to-accent/40"
      />
    </motion.header>
  );
}
