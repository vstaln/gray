import { motion, useReducedMotion, type Variants } from "motion/react";
import { cn } from "../../utils/cn";

type Props = {
  text: string;
  className?: string;
  /** split by "words" or "chars" */
  by?: "words" | "chars";
  delay?: number;
  stagger?: number;
  once?: boolean;
  as?: "h1" | "h2" | "h3" | "p" | "span";
  /** words to render in serif italic accent, matched case-insensitively */
  accent?: string[];
};

/**
 * ReactBits-style SplitText: staggers each word/char up from a blur.
 */
export function SplitText({
  text,
  className,
  by = "words",
  delay = 0,
  stagger = 0.045,
  once = true,
  as = "span",
  accent = [],
}: Props) {
  const reduce = useReducedMotion();
  const Tag = motion[as] as typeof motion.span;
  const tokens = by === "words" ? text.split(" ") : Array.from(text);

  const container: Variants = {
    hidden: {},
    show: { transition: { staggerChildren: stagger, delayChildren: delay } },
  };
  const item: Variants = {
    hidden: reduce ? { opacity: 0 } : { opacity: 0, y: "0.6em", filter: "blur(10px)", rotateX: -30 },
    show: {
      opacity: 1,
      y: 0,
      filter: "blur(0px)",
      rotateX: 0,
      transition: { duration: 0.9, ease: [0.16, 1, 0.3, 1] },
    },
  };

  return (
    <Tag
      className={cn("inline-block", className)}
      variants={container}
      initial="hidden"
      whileInView="show"
      viewport={{ once, amount: 0.5 }}
      aria-label={text}
      style={{ perspective: 800 }}
    >
      {tokens.map((tok, i) => {
        const isAccent = accent.some((a) => a.toLowerCase() === tok.replace(/[.,]/g, "").toLowerCase());
        return (
          <span key={i} className="inline-block overflow-visible align-baseline" style={{ whiteSpace: "pre" }}>
            <motion.span
              variants={item}
              className={cn("inline-block will-change-transform", isAccent && "serif-it text-accent")}
              style={{ transformOrigin: "50% 100%" }}
            >
              {tok}
            </motion.span>
            {by === "words" && i < tokens.length - 1 ? " " : ""}
          </span>
        );
      })}
    </Tag>
  );
}
