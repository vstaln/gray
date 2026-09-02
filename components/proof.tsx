import { Terminal } from "@/components/terminal";

const facts = [
  ["Loop", "ReAct — streams text, calls tools, feeds results back"],
  ["Tools", "bash · read · write · edit · grep · find · ls"],
  ["Sessions", "JSONL on disk, parent-id branching, gray -c to resume"],
  ["Transport", "OpenAI-compatible SSE with typed events and retries"],
  ["Context", "auto-compacts at window − 16k, /compact to force"],
  ["Prompt", "identity + guidelines, editable at ~/.gray/AGENTS.md"],
] as const;

export function Proof() {
  return (
    <section className="relative border-t border-ink-700 starfield">
      <div className="mx-auto grid max-w-[1200px] gap-14 px-6 py-24 lg:grid-cols-[1fr_1fr] lg:items-center">
        <div>
          <p className="eyebrow">The loop</p>
          <h2 className="display mt-4 max-w-[14ch] text-[clamp(2.5rem,5vw,4.5rem)]">
            You watch it think.
          </h2>
          <p className="mt-6 max-w-[46ch] text-dim">
            Text deltas, tool calls and usage all arrive as typed events over SSE, so the terminal
            shows the turn as it happens. Ctrl-C cancels mid-turn and still persists what reached
            memory.
          </p>

          <dl className="mt-10 divide-y divide-ink-700 border-y border-ink-700">
            {facts.map(([k, v]) => (
              <div key={k} className="flex gap-6 py-3">
                <dt className="eyebrow w-[92px] shrink-0 pt-0.5 text-sand-400">{k}</dt>
                <dd className="text-[15px] text-dim">{v}</dd>
              </div>
            ))}
          </dl>
        </div>

        <Terminal />
      </div>
    </section>
  );
}
