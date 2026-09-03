import { motion, type HTMLMotionProps } from "motion/react";
import type { MouseEvent, ReactNode } from "react";
import { cn } from "../../utils/cn";

type Props = HTMLMotionProps<"article"> & {
  children: ReactNode;
  className?: string;
};

/**
 * ReactBits-style SpotlightCard. Sets --mx/--my for the .spotlight border/glow.
 */
export function SpotlightCard({ children, className, onMouseMove, ...rest }: Props) {
  const handleMove = (e: MouseEvent<HTMLElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    e.currentTarget.style.setProperty("--mx", `${e.clientX - r.left}px`);
    e.currentTarget.style.setProperty("--my", `${e.clientY - r.top}px`);
    onMouseMove?.(e as never);
  };
  return (
    <motion.article
      onMouseMove={handleMove}
      className={cn("spotlight group relative overflow-hidden rounded-md bg-ink-900", className)}
      {...rest}
    >
      {children}
    </motion.article>
  );
}
