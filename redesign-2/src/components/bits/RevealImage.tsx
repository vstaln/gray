import { motion, useReducedMotion, useScroll, useTransform } from "motion/react";
import { useRef } from "react";
import { cn } from "../../utils/cn";

type Props = {
  src: string;
  alt: string;
  className?: string;
  imgClassName?: string;
  /** vertical parallax travel in px */
  parallax?: number;
  /** reveal direction */
  from?: "bottom" | "left" | "right" | "top";
  delay?: number;
};

const clips: Record<NonNullable<Props["from"]>, [string, string]> = {
  bottom: ["inset(100% 0 0 0)", "inset(0 0 0 0)"],
  top: ["inset(0 0 100% 0)", "inset(0 0 0 0)"],
  left: ["inset(0 100% 0 0)", "inset(0 0 0 0)"],
  right: ["inset(0 0 0 100%)", "inset(0 0 0 0)"],
};

/**
 * Dithered image with: clip-path wipe on enter, subtle scroll parallax,
 * slow drift, and a scale on group hover. Screen-blended per the original .dither.
 */
export function RevealImage({ src, alt, className, imgClassName, parallax = 40, from = "bottom", delay = 0 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const reduce = useReducedMotion();
  const { scrollYProgress } = useScroll({ target: ref, offset: ["start end", "end start"] });
  const y = useTransform(scrollYProgress, [0, 1], [reduce ? 0 : -parallax, reduce ? 0 : parallax]);
  const [hidden, shown] = clips[from];

  return (
    <div ref={ref} className={cn("absolute inset-0 overflow-hidden", className)} aria-hidden>
      <motion.div
        initial={{ clipPath: hidden, opacity: 0.6 }}
        whileInView={{ clipPath: shown, opacity: 1 }}
        viewport={{ once: true, amount: 0.25 }}
        transition={{ duration: 1.4, ease: [0.16, 1, 0.3, 1], delay }}
        className="absolute inset-[-8%]"
      >
        <motion.img
          src={src}
          alt={alt}
          loading="lazy"
          style={{ y }}
          className={cn(
            "dither h-full w-full object-cover opacity-55 transition-[transform,opacity,filter] duration-[1400ms] ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:scale-[1.05] group-hover:opacity-75",
            imgClassName,
          )}
        />
      </motion.div>
    </div>
  );
}
