import { motion } from "motion/react";
import type { ReactNode } from "react";
import { cn } from "../utils/cn";

type Props = {
  eyebrow: string;
  index?: string;
  children: ReactNode;
  className?: string;
  align?: "left" | "center";
};

/**
 * Hermes-style split eyebrow: hairline, section index left, label right, then the heading.
 */
export function SectionHead({ eyebrow, index, children, className, align = "left" }: Props) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, amount: 0.5 }}
      transition={{ duration: 0.8, ease: [0.16, 1, 0.3, 1] }}
      className={cn(align === "center" ? "mx-auto max-w-2xl text-center" : "max-w-2xl", className)}
    >
      <div className={cn("mb-8 flex items-center gap-4", align === "center" && "justify-center")}>
        {index ? <span className="mono text-[11px] tracking-[0.14em] text-accent">{index}</span> : null}
        <motion.span
          initial={{ scaleX: 0 }}
          whileInView={{ scaleX: 1 }}
          viewport={{ once: true }}
          transition={{ duration: 1.1, ease: [0.16, 1, 0.3, 1], delay: 0.15 }}
          className="h-px w-12 origin-left bg-ink-600"
        />
        <span className="mono text-[11px] uppercase tracking-[0.18em] text-ink-400">{eyebrow}</span>
      </div>
      {children}
    </motion.div>
  );
}
