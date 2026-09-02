import { Backdrop } from "@/components/backdrop";
import { CopyCommand } from "@/components/copy-command";
import { install } from "@/lib/site";

export function Hero() {
  return (
    <section className="relative isolate flex min-h-[92svh] items-end overflow-hidden pb-20 pt-32">
      <Backdrop src="/space/carina-plate.jpg" opacity={0.62} blur={0} fade="bottom" drift />

      {/* diffusion pass: the plate again, blown out and blurred into the black */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 bg-cover bg-center opacity-30 mix-blend-screen blur-[90px]"
        style={{
          backgroundImage: "url(/space/carina-plate.jpg)",
          maskImage: "linear-gradient(to bottom, black 0%, transparent 78%)",
          WebkitMaskImage: "linear-gradient(to bottom, black 0%, transparent 78%)",
        }}
      />

      <div className="relative mx-auto w-full max-w-[1200px] px-6">
        <p className="eyebrow motion-safe:animate-[fade-up_0.8s_var(--ease-out-expo)_both]">
          Rust · OpenAI-compatible · MIT
        </p>

        <h1 className="display mt-6 max-w-[16ch] text-[clamp(3.5rem,10vw,8.5rem)] motion-safe:animate-[fade-up_0.9s_var(--ease-out-expo)_0.08s_both]">
          The agent that fits in <span className="italic text-sand-300">one binary</span>.
        </h1>

        <p className="mt-8 max-w-[52ch] text-lg text-dim motion-safe:animate-[fade-up_0.9s_var(--ease-out-expo)_0.16s_both]">
          gray runs tools, edits code, and streams from any model provider. No runtime, no
          node_modules, no dashboard. It starts at a prompt and gets out of the way.
        </p>

        <div className="mt-10 max-w-[640px] motion-safe:animate-[fade-up_0.9s_var(--ease-out-expo)_0.24s_both]">
          <CopyCommand command={install.unix} label="Install" />
          <p className="mt-3 font-mono text-[11px] text-dimmer">
            macOS · Linux · WSL — Windows PowerShell and source builds below
          </p>
        </div>
      </div>
    </section>
  );
}
