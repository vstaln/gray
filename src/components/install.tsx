"use client";

import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { InstallCommand } from "./install-command";
import { Reveal, RevealItem } from "./reveal";

const targets = [
  {
    id: "unix",
    label: "macOS / Linux",
    prompt: "$",
    command: "curl -fsSL https://gray.alignment.id/install.sh | sh",
    note: "Stable channel. Detects your platform and drops the binary on your PATH.",
  },
  {
    id: "beta",
    label: "Beta",
    prompt: "$",
    command: "curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta",
    note: "Rebuilt on every push to main. Verified by the in-app update check.",
  },
  {
    id: "windows",
    label: "Windows",
    prompt: ">",
    command: "iwr https://gray.alignment.id/install.ps1 -UseBasicParsing | iex",
    note: "Runs through WSL. The script checks for it and guides you if it is missing.",
  },
  {
    id: "source",
    label: "From source",
    prompt: "$",
    command: "cargo build --release -p gray",
    note: "Needs a Rust toolchain. Produces the same statically linked binary.",
  },
];

const commands = [
  ["/connect", "set up provider and API key"],
  ["/model", "switch model"],
  ["/thinking", "reasoning effort"],
  ["/context", "set context window, e.g. 128k or auto"],
  ["/resume", "resume a conversation"],
  ["/new", "new conversation"],
  ["/compact", "summarize context"],
  ["/usage", "session tokens and cost"],
];

export function Install() {
  const [active, setActive] = useState(targets[0]);

  return (
    <section id="install" className="mx-auto max-w-7xl scroll-mt-24 px-5 py-28 sm:px-8 sm:py-36">
      <Reveal className="max-w-2xl">
        <RevealItem as="h2" className="display text-[clamp(2rem,4.5vw,3.5rem)] font-semibold leading-[1.02] text-ink-50">
          One command. No prerequisites.
        </RevealItem>
        <RevealItem as="p" className="prose-tight mt-4 max-w-[52ch] text-[16px] leading-relaxed text-ink-300">
          Builds are published to gray.alignment.id/dl for every platform and verified by the
          in-app update check.
        </RevealItem>
      </Reveal>

      <motion.div
        initial={{ opacity: 0, y: 24 }}
        whileInView={{ opacity: 1, y: 0 }}
        viewport={{ once: true, amount: 0.4 }}
        transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1] }}
        className="mt-12 rounded-md border border-ink-800 bg-ink-900/60"
      >
        <div role="tablist" aria-label="Install target" className="flex overflow-x-auto border-b border-ink-800 px-2">
          {targets.map((t) => {
            const selected = t.id === active.id;
            return (
              <button
                key={t.id}
                role="tab"
                type="button"
                aria-selected={selected}
                onClick={() => setActive(t)}
                className={`focus-ring relative shrink-0 px-4 py-3.5 text-[13.5px] transition-colors duration-200 ${
                  selected ? "text-ink-50" : "text-ink-400 hover:text-ink-200"
                }`}
              >
                {t.label}
                {selected ? (
                  <motion.span
                    layoutId="install-tab"
                    className="absolute inset-x-3 -bottom-px h-px bg-accent"
                    transition={{ type: "spring", stiffness: 420, damping: 38 }}
                  />
                ) : null}
              </button>
            );
          })}
        </div>

        <div className="p-4 sm:p-6">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={active.id}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -8 }}
              transition={{ duration: 0.25, ease: [0.16, 1, 0.3, 1] }}
            >
              <InstallCommand size="lg" prompt={active.prompt} command={active.command} />
              <p className="mt-3 text-[13.5px] text-ink-400">{active.note}</p>
            </motion.div>
          </AnimatePresence>
        </div>
      </motion.div>

      <div className="mt-20 grid grid-cols-1 gap-10 lg:grid-cols-12">
        <Reveal className="lg:col-span-4">
          <RevealItem as="h3" className="display text-[26px] font-semibold leading-tight text-ink-50 sm:text-[30px]">
            Then just run it.
          </RevealItem>
          <RevealItem as="p" className="prose-tight mt-3 max-w-[40ch] text-[15.5px] leading-relaxed text-ink-300">
            First run drops you straight at the prompt. Nothing is forced at boot; configure
            whenever you feel like it.
          </RevealItem>
          <RevealItem className="mt-6">
            <InstallCommand prompt="❯" command="gray" className="max-w-xs" />
          </RevealItem>
        </Reveal>

        <Reveal as="ul" amount={0.2} className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:col-span-8">
          {commands.map(([cmd, desc]) => (
            <RevealItem
              key={cmd}
              as="li"
              className="group flex items-baseline gap-4 rounded-sm border border-transparent px-4 py-3.5 transition-colors duration-200 hover:border-ink-800 hover:bg-ink-900/70"
            >
              <code className="mono shrink-0 text-[14px] text-accent">{cmd}</code>
              <span className="text-[14px] text-ink-300 transition-colors duration-200 group-hover:text-ink-100">
                {desc}
              </span>
            </RevealItem>
          ))}
        </Reveal>
      </div>
    </section>
  );
}
