"use client";

import { GithubLogoIcon } from "@phosphor-icons/react";
import { motion, useMotionValueEvent, useScroll } from "motion/react";
import { useState } from "react";

const links = [
  { href: "#features", label: "What it does" },
  { href: "#loop", label: "The loop" },
  { href: "#install", label: "Install" },
  { href: "#pricing", label: "Pricing" },
];

export function Nav() {
  const { scrollY } = useScroll();
  const [scrolled, setScrolled] = useState(false);

  useMotionValueEvent(scrollY, "change", (v) => setScrolled(v > 24));

  return (
    <motion.header
      initial={{ y: -16, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      transition={{ duration: 0.6, ease: [0.16, 1, 0.3, 1] }}
      className="fixed inset-x-0 top-0 z-40"
    >
      <div
        className={`mx-auto flex h-16 max-w-7xl items-center justify-between px-5 transition-all duration-500 sm:px-8 ${
          scrolled ? "mt-3" : "mt-0"
        }`}
      >
        <div
          className={`flex h-12 w-full items-center justify-between rounded-md border px-3 transition-all duration-500 ${
            scrolled
              ? "border-ink-800 bg-ink-900/70 backdrop-blur-xl backdrop-saturate-150"
              : "border-transparent bg-transparent"
          }`}
        >
          <a href="#" className="focus-ring display rounded-xs px-2 text-[19px] font-semibold text-ink-50">
            gray
          </a>

          <nav aria-label="Primary" className="hidden items-center gap-1 md:flex">
            {links.map((l) => (
              <a
                key={l.href}
                href={l.href}
                className="focus-ring rounded-xs px-3 py-1.5 text-[13.5px] text-ink-300 transition-colors duration-200 hover:text-ink-50"
              >
                {l.label}
              </a>
            ))}
          </nav>

          <div className="flex items-center gap-2">
            <a
              href="https://github.com/vstaln/gray"
              target="_blank"
              rel="noreferrer"
              className="focus-ring inline-flex h-8 items-center gap-2 rounded-xs px-2.5 text-[13px] text-ink-300 transition-colors duration-200 hover:text-ink-50"
            >
              <GithubLogoIcon size={16} weight="fill" />
              <span className="hidden sm:inline">GitHub</span>
            </a>
            <a
              href="#install"
              className="focus-ring inline-flex h-8 items-center rounded-xs bg-ink-50 px-3 text-[13px] font-medium text-ink-950 transition-all duration-200 hover:bg-white active:scale-[0.98]"
            >
              Install
            </a>
          </div>
        </div>
      </div>
    </motion.header>
  );
}
