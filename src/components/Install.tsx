import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { DecryptedText } from "./bits/DecryptedText";
import { SplitText } from "./bits/SplitText";
import { InstallCommand } from "./InstallCommand";
import { SectionHead } from "./SectionHead";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./ui/tabs";

const targets = [
  { id: "unix", label: "macOS / Linux", cmd: "curl -fsSL https://gray.alignment.id/install.sh | sh", note: "Installs the latest stable to ~/.local/bin and verifies the checksum." },
  { id: "beta", label: "Beta", cmd: "curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta", note: "Bleeding edge. Beta rebuilds on every push to main." },
  { id: "win", label: "Windows", cmd: "iwr https://gray.alignment.id/install.ps1 -UseBasicParsing | iex", note: "Runs through WSL — the script checks and guides you." },
  { id: "src", label: "From source", cmd: "cargo build --release -p gray", note: "Stable Rust toolchain. Nothing else." },
];

const commands = [
  ["/connect", "setup provider & API key"],
  ["/model", "switch model"],
  ["/thinking", "reasoning effort"],
  ["/context", "set context window (e.g. 128k, auto)"],
  ["/resume", "resume conversation"],
  ["/new", "new conversation"],
  ["/compact", "summarize context"],
  ["/usage", "session tokens & cost"],
];

export function Install() {
  const [tab, setTab] = useState("unix");
  const active = targets.find((t) => t.id === tab)!;

  return (
    <section id="install" className="relative scroll-mt-24 overflow-hidden">
      {/* ghosted moon in the background */}
      <div className="pointer-events-none absolute -left-[20%] top-[10%] -z-10 aspect-square w-[70vw] max-w-[900px] opacity-30" aria-hidden>
        <img
          src="/space/moon.jpg"
          alt=""
          className="dither h-full w-full object-cover [mask-image:radial-gradient(ellipse_at_center,black_20%,transparent_65%)]"
        />
      </div>

      <div className="mx-auto max-w-7xl px-5 py-28 sm:px-8 sm:py-36">
        <div className="grid grid-cols-1 gap-14 lg:grid-cols-12 lg:gap-10">
          <div className="lg:col-span-6">
            <SectionHead eyebrow="Install" index="§03">
              <h2 className="display text-[clamp(2rem,4.8vw,3.75rem)] font-semibold leading-[1.02] text-ink-50">
                <SplitText text="One command." /> <br className="hidden sm:block" />
                <SplitText text="No prerequisites." accent={["prerequisites."]} delay={0.2} />
              </h2>
            </SectionHead>

            <motion.div
              initial={{ opacity: 0, y: 20 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true, amount: 0.4 }}
              transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1], delay: 0.15 }}
              className="mt-10"
            >
              <Tabs value={tab} onValueChange={setTab}>
                <TabsList className="flex-wrap h-auto">
                  {targets.map((t) => (
                    <TabsTrigger key={t.id} value={t.id}>
                      {t.label}
                    </TabsTrigger>
                  ))}
                </TabsList>
                {targets.map((t) => (
                  <TabsContent key={t.id} value={t.id} forceMount hidden={t.id !== tab}>
                    <AnimatePresence mode="wait">
                      {t.id === tab && (
                        <motion.div
                          key={t.id}
                          initial={{ opacity: 0, y: 8, filter: "blur(6px)" }}
                          animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                          exit={{ opacity: 0, y: -8, filter: "blur(6px)" }}
                          transition={{ duration: 0.45, ease: [0.16, 1, 0.3, 1] }}
                        >
                          <InstallCommand size="lg" command={t.cmd} />
                          <p className="prose-tight mt-3 text-[13.5px] leading-relaxed text-ink-400">{active.note}</p>
                        </motion.div>
                      )}
                    </AnimatePresence>
                  </TabsContent>
                ))}
              </Tabs>

              <p className="prose-tight mt-8 max-w-[52ch] text-[14px] leading-relaxed text-ink-400">
                Builds are published to{" "}
                <span className="mono text-ink-200">gray.alignment.id/dl</span> and verified by the in-app
                update check.
              </p>
            </motion.div>
          </div>

          <div className="lg:col-span-6">
            <motion.div
              initial={{ opacity: 0, y: 24 }}
              whileInView={{ opacity: 1, y: 0 }}
              viewport={{ once: true, amount: 0.3 }}
              transition={{ duration: 0.9, ease: [0.16, 1, 0.3, 1], delay: 0.25 }}
              className="rounded-md border border-ink-800 bg-ink-900/60 backdrop-blur"
            >
              <div className="flex items-center justify-between border-b border-ink-800 px-5 py-3">
                <span className="mono text-[10.5px] uppercase tracking-[0.16em] text-ink-500">Then</span>
                <span className="serif-it text-[15px] text-ink-400">nothing forced at boot</span>
              </div>
              <div className="px-5 pt-5">
                <div className="mono flex items-center gap-3 text-[15px] text-ink-50">
                  <span className="text-accent">❯</span>
                  <span>gray</span>
                  <span className="inline-block h-4 w-2 animate-caret bg-accent" />
                </div>
                <p className="prose-tight mt-3 text-[13.5px] leading-relaxed text-ink-400">
                  First run drops you straight at the prompt. Configure whenever you feel like it:
                </p>
              </div>
              <ul className="mt-4 divide-y divide-ink-800/80 border-t border-ink-800/80">
                {commands.map(([c, d], i) => (
                  <motion.li
                    key={c}
                    initial={{ opacity: 0, x: -10 }}
                    whileInView={{ opacity: 1, x: 0 }}
                    viewport={{ once: true }}
                    transition={{ duration: 0.5, delay: 0.05 * i, ease: [0.16, 1, 0.3, 1] }}
                    className="group flex items-baseline gap-5 px-5 py-3 transition-colors hover:bg-ink-850/80"
                  >
                    <span className="mono w-[104px] shrink-0 text-[13px] text-accent">
                      <DecryptedText text={c} trigger="hover" speed={22} />
                    </span>
                    <span className="text-[13.5px] text-ink-300 transition-colors group-hover:text-ink-100">{d}</span>
                    <span className="ml-auto h-1 w-1 rounded-full bg-ink-700 transition-all duration-500 group-hover:bg-accent group-hover:shadow-[0_0_8px_rgba(217,176,97,0.8)]" />
                  </motion.li>
                ))}
              </ul>
            </motion.div>
          </div>
        </div>
      </div>
    </section>
  );
}
