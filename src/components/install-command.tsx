"use client";

import { CheckIcon, CopyIcon } from "@phosphor-icons/react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useState } from "react";

export function InstallCommand({
  command,
  prompt = "$",
  size = "md",
  className = "",
}: {
  command: string;
  prompt?: string;
  size?: "md" | "lg";
  className?: string;
}) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1800);
    return () => clearTimeout(t);
  }, [copied]);

  const copy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
    } catch {
      // Clipboard can be unavailable in insecure contexts; fail quietly.
    }
  }, [command]);

  const text = size === "lg" ? "text-[13px] sm:text-[15px]" : "text-[13px]";
  const pad = size === "lg" ? "py-3.5 pl-4 pr-2" : "py-2.5 pl-3.5 pr-1.5";

  return (
    <div
      className={`group flex items-center gap-3 rounded-sm border border-ink-700 bg-ink-900/80 ${pad} ${className}`}
    >
      <span className={`mono select-none text-accent ${text}`} aria-hidden>
        {prompt}
      </span>
      <code className={`mono min-w-0 flex-1 overflow-x-auto whitespace-nowrap text-ink-100 ${text}`}>
        {command}
      </code>
      <motion.button
        type="button"
        onClick={copy}
        whileTap={{ scale: 0.94 }}
        aria-label={copied ? "Copied" : "Copy command"}
        className="focus-ring relative grid h-9 w-9 shrink-0 place-items-center rounded-xs text-ink-300 transition-colors duration-200 hover:bg-ink-800 hover:text-ink-50"
      >
        <AnimatePresence mode="wait" initial={false}>
          {copied ? (
            <motion.span
              key="check"
              initial={{ scale: 0.6, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: 0.6, opacity: 0 }}
              transition={{ duration: 0.18 }}
              className="text-accent"
            >
              <CheckIcon size={16} weight="bold" />
            </motion.span>
          ) : (
            <motion.span
              key="copy"
              initial={{ scale: 0.6, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: 0.6, opacity: 0 }}
              transition={{ duration: 0.18 }}
            >
              <CopyIcon size={16} />
            </motion.span>
          )}
        </AnimatePresence>
      </motion.button>
    </div>
  );
}
