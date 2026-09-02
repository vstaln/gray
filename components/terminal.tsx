"use client";

import { useEffect, useRef, useState } from "react";

type Line = { kind: "user" | "text" | "tool" | "dim" | "gap"; text?: string };

/**
 * Replays a real gray session shape: ❯ prompt, streaming assistant text, a tool
 * call, then the token footer. Typed out rather than videoed so it stays sharp,
 * copyable and ~0 bytes.
 */
const SCRIPT: Line[] = [
  { kind: "user", text: "find where the context window is resolved" },
  { kind: "gap" },
  { kind: "text", text: "Looking at the config path first." },
  { kind: "gap" },
  { kind: "tool", text: "grep  context_window  crates/" },
  { kind: "dim", text: "  4 matches · crates/gray/src/config.rs" },
  { kind: "gap" },
  { kind: "text", text: "Resolution order is --context-window > GRAY_CONTEXT_WINDOW > the" },
  { kind: "text", text: "auto-fetched provider value > a per-model fallback." },
  { kind: "gap" },
  { kind: "dim", text: "  ⬡ 2.1k in · 340 out · 12% of 128k" },
];

const CHAR_MS = 16;
const LINE_MS = 260;

export function Terminal() {
  const [shown, setShown] = useState<Line[]>([]);
  const [partial, setPartial] = useState("");
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    let cancelled = false;

    async function type() {
      for (const [i, line] of SCRIPT.entries()) {
        if (cancelled) return;
        if (line.kind === "gap" || !line.text) {
          setShown(SCRIPT.slice(0, i + 1));
          await sleep(LINE_MS / 2);
          continue;
        }
        for (let c = 1; c <= line.text.length; c++) {
          if (cancelled) return;
          setPartial(line.text.slice(0, c));
          await sleep(CHAR_MS);
        }
        setPartial("");
        setShown(SCRIPT.slice(0, i + 1));
        await sleep(LINE_MS);
      }
    }

    // Reduced motion gets the finished transcript, not an empty box.
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      const id = setTimeout(() => setShown(SCRIPT), 0);
      return () => clearTimeout(id);
    }

    const io = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        io.disconnect();
        void type();
      },
      { threshold: 0.3 },
    );
    io.observe(el);

    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, []);

  const typingKind = SCRIPT[shown.length]?.kind ?? "text";

  return (
    <div
      ref={ref}
      className="relative overflow-hidden border border-ink-700 bg-ink-900/90 shadow-[0_40px_120px_-30px_rgba(0,0,0,0.9)] backdrop-blur-sm"
    >
      <div className="flex items-center gap-3 border-b border-ink-700 px-5 py-3">
        <span className="size-2 rounded-full bg-ink-600" />
        <span className="size-2 rounded-full bg-ink-600" />
        <span className="size-2 rounded-full bg-ink-600" />
        <span className="eyebrow ml-2">gray — ~/src/gray</span>
      </div>

      <div className="min-h-[340px] p-6 font-mono text-[13px] leading-[1.9]">
        {shown.map((l, idx) => (
          <Row key={idx} line={l} />
        ))}
        {partial ? <Row line={{ kind: typingKind, text: partial }} caret /> : null}
      </div>
    </div>
  );
}

function Row({ line, caret }: { line: Line; caret?: boolean }) {
  if (line.kind === "gap") return <div className="h-4" />;
  const tone =
    line.kind === "user"
      ? "text-paper"
      : line.kind === "tool"
        ? "text-sand-300"
        : line.kind === "dim"
          ? "text-dimmer"
          : "text-dim";
  return (
    <div className={tone}>
      {line.kind === "user" ? <span className="mr-2 text-sand-400">❯</span> : null}
      {line.kind === "tool" ? <span className="mr-2 text-ink-500">▸</span> : null}
      <span className="whitespace-pre-wrap">{line.text}</span>
      {caret ? <span className="ml-0.5 animate-[caret_1s_step-end_infinite]">▋</span> : null}
    </div>
  );
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}
