import { Backdrop } from "@/components/backdrop";
import { CopyCommand } from "@/components/copy-command";
import { install, slashCommands } from "@/lib/site";

export function Install() {
  return (
    <section id="install" className="relative isolate overflow-hidden border-t border-ink-700">
      <Backdrop src="/space/earthlimb-plate.jpg" opacity={0.4} blur={2} fade="both" />

      <div className="relative mx-auto max-w-[1200px] px-6 py-24">
        <p className="eyebrow">Install</p>
        <h2 className="display mt-4 max-w-[16ch] text-[clamp(2.5rem,5vw,4.5rem)]">
          One command. No prerequisites.
        </h2>

        <div className="mt-12 grid gap-10 lg:grid-cols-[1.2fr_1fr]">
          <div className="space-y-4">
            <CopyCommand command={install.unix} label="macOS / Linux" />
            <CopyCommand command={install.unixBeta} label="Beta" />
            <CopyCommand command={install.windows} label="Windows" />
            <CopyCommand command={install.source} label="From source" />
            <p className="font-mono text-[11px] leading-relaxed text-dimmer">
              Beta rebuilds on every push to main. Windows runs through WSL — the script checks and
              guides you. Builds are published to gray.alignment.id/dl and verified by the in-app
              update check.
            </p>
          </div>

          <div className="border border-ink-700 bg-ink-900/70 p-6 backdrop-blur-sm">
            <p className="eyebrow">Then</p>
            <p className="mt-4 font-mono text-[13px] text-paper">
              <span className="mr-2 text-sand-400">❯</span>gray
            </p>
            <p className="mt-4 text-[15px] leading-relaxed text-dim">
              First run drops you straight at the prompt — nothing forced at boot. Configure
              whenever you feel like it:
            </p>
            <dl className="mt-6 space-y-2 border-t border-ink-700 pt-6">
              {slashCommands.slice(0, 8).map(([cmd, desc]) => (
                <div key={cmd} className="flex gap-4 font-mono text-[11px]">
                  <dt className="w-[128px] shrink-0 text-sand-400">{cmd}</dt>
                  <dd className="text-dim">{desc}</dd>
                </div>
              ))}
            </dl>
          </div>
        </div>
      </div>
    </section>
  );
}
