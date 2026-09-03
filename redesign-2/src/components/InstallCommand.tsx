import { Check, Copy } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { toast } from "sonner";
import { cn } from "../utils/cn";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

type Props = {
  command: string;
  label?: string;
  className?: string;
  size?: "md" | "lg";
};

export function InstallCommand({ command, label, className, size = "md" }: Props) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      toast("Copied to clipboard", { description: command });
      setTimeout(() => setCopied(false), 1800);
    } catch {
      toast.error("Clipboard blocked — select and copy manually");
    }
  };

  return (
    <div className={cn("group/cmd relative", className)}>
      {label ? (
        <span className="mono mb-2 block text-[10.5px] uppercase tracking-[0.16em] text-ink-500">{label}</span>
      ) : null}
      <button
        type="button"
        onClick={copy}
        className={cn(
          "focus-ring mono relative flex w-full items-center gap-3 overflow-hidden rounded-sm border border-ink-700/80 bg-ink-900/80 text-left text-ink-100 backdrop-blur transition-[border-color,box-shadow] duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] hover:border-ink-500 hover:shadow-[0_0_0_1px_rgba(217,176,97,0.18),0_20px_50px_-30px_rgba(217,176,97,0.4)]",
          size === "lg" ? "px-4 py-3.5 text-[13.5px] sm:px-5 sm:text-[15px]" : "px-3.5 py-2.5 text-[12.5px] sm:text-[13px]",
        )}
        aria-label={`Copy: ${command}`}
      >
        {/* animated sheen */}
        <span
          aria-hidden
          className="pointer-events-none absolute inset-0 -translate-x-full bg-gradient-to-r from-transparent via-white/[0.06] to-transparent transition-transform duration-[1200ms] ease-[cubic-bezier(0.16,1,0.3,1)] group-hover/cmd:translate-x-full"
        />
        <span className="select-none text-accent">$</span>
        <span className="min-w-0 flex-1 truncate">{command}</span>
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="relative grid h-7 w-7 shrink-0 place-items-center rounded-xs border border-ink-700 bg-ink-850 text-ink-400 transition-colors group-hover/cmd:border-ink-600 group-hover/cmd:text-ink-100">
              <AnimatePresence mode="wait" initial={false}>
                {copied ? (
                  <motion.span
                    key="ok"
                    initial={{ scale: 0.4, opacity: 0, rotate: -30 }}
                    animate={{ scale: 1, opacity: 1, rotate: 0 }}
                    exit={{ scale: 0.4, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 500, damping: 26 }}
                    className="text-accent"
                  >
                    <Check className="h-3.5 w-3.5" />
                  </motion.span>
                ) : (
                  <motion.span
                    key="copy"
                    initial={{ scale: 0.4, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0.4, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 500, damping: 26 }}
                  >
                    <Copy className="h-3.5 w-3.5" />
                  </motion.span>
                )}
              </AnimatePresence>
            </span>
          </TooltipTrigger>
          <TooltipContent>{copied ? "Copied" : "Copy"}</TooltipContent>
        </Tooltip>
      </button>
    </div>
  );
}
