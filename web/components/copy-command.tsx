"use client";

import { useState } from "react";
import { cn } from "@/lib/cn";

export function CopyCommand({
  command,
  label,
  className,
}: {
  command: string;
  label?: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      // clipboard blocked (insecure context / permission) — the text is still selectable
    }
  }

  return (
    <div
      className={cn(
        "group relative flex items-center gap-4 border border-ink-600 bg-ink-900/80 px-5 py-4 backdrop-blur-sm transition-colors hover:border-ink-500",
        className,
      )}
    >
      {label ? <span className="eyebrow hidden shrink-0 sm:block">{label}</span> : null}
      <code className="min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-[13px] text-paper">
        <span className="mr-2 select-none text-sand-400">$</span>
        {command}
      </code>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? "Copied" : "Copy command"}
        className="eyebrow shrink-0 border border-ink-600 px-3 py-1.5 text-dim transition-colors hover:border-sand-400/50 hover:text-sand-300"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}
