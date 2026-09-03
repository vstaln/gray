import { motion, useMotionValue, useSpring } from "motion/react";
import { useRef, type MouseEvent, type ReactNode } from "react";
import { cn } from "../../utils/cn";

type Props = {
  children: ReactNode;
  className?: string;
  /** pull strength 0..1 */
  strength?: number;
  /** activation radius in px beyond element bounds */
  padding?: number;
};

/**
 * ReactBits-style Magnet: element is pulled toward the cursor when near.
 */
export function Magnet({ children, className, strength = 0.35, padding = 40 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const mx = useMotionValue(0);
  const my = useMotionValue(0);
  const x = useSpring(mx, { stiffness: 220, damping: 18, mass: 0.5 });
  const y = useSpring(my, { stiffness: 220, damping: 18, mass: 0.5 });

  const onMove = (e: MouseEvent<HTMLDivElement>) => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const dx = e.clientX - cx;
    const dy = e.clientY - cy;
    const inside = Math.abs(dx) < r.width / 2 + padding && Math.abs(dy) < r.height / 2 + padding;
    if (inside) {
      mx.set(dx * strength);
      my.set(dy * strength);
    } else {
      mx.set(0);
      my.set(0);
    }
  };
  const reset = () => {
    mx.set(0);
    my.set(0);
  };

  return (
    <div
      ref={ref}
      onMouseMove={onMove}
      onMouseLeave={reset}
      className={cn("inline-block", className)}
      style={{ padding }}
    >
      <motion.div style={{ x, y }} className="inline-block">
        {children}
      </motion.div>
    </div>
  );
}
