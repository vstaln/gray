import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useEffect, useState } from "react";
import { cn } from "../utils/cn";

type Line =
  | { kind: "prompt"; text: string }
  | { kind: "text"; text: string }
  | { kind: "tool"; name: string; arg: string }
  | { kind: "result"; text: string }
  | { kind: "usage"; text: string };

const script: Line[] = [
  { kind: "prompt", text: "why does the session store fail to resume after ctrl-c?" },
  { kind: "text", text: "Let me look at how sessions are appended." },
  { kind: "tool", name: "grep", arg: "\"parent_id\" crates/gray-session/src" },
  { kind: "result", text: "store.rs:41  fn append(&mut self, ev: Event) -> io::Result<()>" },
  { kind: "tool", name: "read", arg: "crates/gray-session/src/store.rs:30-70" },
  { kind: "text", text: "The writer buffers and only flushes on drop. An interrupt skips drop." },
  { kind: "tool", name: "edit", arg: "store.rs  +flush() after every append" },
  { kind: "result", text: "1 hunk applied · 3 lines" },
  { kind: "tool", name: "bash", arg: "cargo test -p gray-session" },
  { kind: "result", text: "test result: ok. 14 passed; 0 failed" },
  { kind: "text", text: "Fixed. Interrupted turns now persist what reached memory." },
  { kind: "usage", text: "in 3.2k · out 612 · $0.004 · 6.1s" },
];

/**
 * Loops through a fake ReAct turn as a stream of typed events,
 * mirroring the SSE event model the binary actually uses.
 */
export function Terminal({ className, compact = false }: { className?: string; compact?: boolean }) {
  const reduce = useReducedMotion();
  const [count, setCount] = useState(reduce ? script.length : 0);
  const [typed, setTyped] = useState("");

  useEffect(() => {
    if (reduce) return;
    let cancelled = false;
    let t: number;

    const step = (i: number) => {
      if (cancelled) return;
      if (i >= script.length) {
        t = window.setTimeout(() => {
          setCount(0);
          setTyped("");
          step(0);
        }, 4200);
        return;
      }
      const line = script[i];
      if (line.kind === "text" || line.kind === "prompt") {
        // type it out
        let j = 0;
        const type = () => {
          if (cancelled) return;
          j += 1;
          setTyped(line.text.slice(0, j));
          if (j < line.text.length) {
            t = window.setTimeout(type, line.kind === "prompt" ? 26 : 14);
          } else {
            setCount(i + 1);
            setTyped("");
            t = window.setTimeout(() => step(i + 1), 360);
          }
        };
        type();
      } else {
        setCount(i + 1);
        t = window.setTimeout(() => step(i + 1), line.kind === "tool" ? 520 : 700);
      }
    };
    t = window.setTimeout(() => step(0), 900);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [reduce]);

  const shown = script.slice(0, count);
  const current = count < script.length ? script[count] : null;
  const isTyping = current && (current.kind === "text" || current.kind === "prompt") && typed.length > 0;

  return (
    <div
      className={cn(
        "mono relative overflow-hidden rounded-md border border-ink-800 bg-ink-950/90 text-[12.5px] leading-relaxed shadow-[0_40px_120px_-40px_rgba(0,0,0,0.8),inset_0_1px_0_rgba(255,255,255,0.04)] backdrop-blur",
        className,
      )}
    >
      {/* title bar */}
      <div className="flex items-center justify-between border-b border-ink-800/80 bg-ink-900/60 px-3.5 py-2">
        <div className="flex items-center gap-1.5">
          <span className="h-2.5 w-2.5 rounded-full bg-ink-700" />
          <span className="h-2.5 w-2.5 rounded-full bg-ink-700" />
          <span className="h-2.5 w-2.5 rounded-full bg-ink-700" />
        </div>
        <span className="text-[10.5px] uppercase tracking-[0.16em] text-ink-500">gray · sse</span>
        <span className="flex items-center gap-1.5 text-[10.5px] text-ink-500">
          <span className="h-1.5 w-1.5 animate-pulse-dot rounded-full bg-accent" />
          live
        </span>
      </div>

      <div className={cn("space-y-1.5 px-4 py-4", compact ? "min-h-[280px]" : "min-h-[380px]")}>
        <AnimatePresence initial={false}>
          {shown.map((l, i) => (
            <motion.div
              key={i}
              initial={{ opacity: 0, y: 6, filter: "blur(4px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              transition={{ duration: 0.5, ease: [0.16, 1, 0.3, 1] }}
            >
              <LineView line={l} />
            </motion.div>
          ))}
        </AnimatePresence>
        {isTyping && current ? (
          <div>
            <LineView line={{ ...current, text: typed } as Line} caret />
          </div>
        ) : count < script.length ? (
          <div className="flex items-center gap-2 text-ink-500">
            <span className="inline-block h-3.5 w-1.5 animate-caret bg-accent" />
          </div>
        ) : null}
      </div>

      {/* bottom status line like the REPL */}
      <div className="flex items-center justify-between border-t border-ink-800/80 px-3.5 py-1.5 text-[10.5px] text-ink-500">
        <span>~/.gray/sessions/2026-04-12.jsonl</span>
        <span className="hidden sm:inline">ctx 41% · /compact</span>
      </div>
    </div>
  );
}

function LineView({ line, caret }: { line: Line; caret?: boolean }) {
  const Caret = caret ? <span className="ml-0.5 inline-block h-3.5 w-1.5 translate-y-0.5 animate-caret bg-accent" /> : null;
  switch (line.kind) {
    case "prompt":
      return (
        <p className="text-ink-50">
          <span className="mr-2 text-accent">❯</span>
          {line.text}
          {Caret}
        </p>
      );
    case "text":
      return (
        <p className="text-ink-200">
          {line.text}
          {Caret}
        </p>
      );
    case "tool":
      return (
        <p className="flex items-baseline gap-2">
          <span className="rounded-xs border border-accent/30 bg-accent/10 px-1.5 py-px text-[10.5px] uppercase tracking-[0.12em] text-accent">
            {line.name}
          </span>
          <span className="truncate text-ink-300">{line.arg}</span>
        </p>
      );
    case "result":
      return <p className="border-l border-ink-700 pl-3 text-ink-400">{line.text}</p>;
    case "usage":
      return <p className="pt-1 text-[11px] uppercase tracking-[0.12em] text-ink-500">{line.text}</p>;
  }
}
