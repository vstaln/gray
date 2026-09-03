import { motion, useMotionValue, useSpring, useTransform } from "motion/react";
import type { MouseEvent, ReactNode } from "react";
import { cn } from "../../utils/cn";

type Props = {
  children: ReactNode;
  className?: string;
  /** max tilt in degrees */
  amount?: number;
  scale?: number;
};

/**
 * ReactBits-style TiltedCard: 3D tilt following the cursor with spring easing.
 */
export function TiltedCard({ children, className, amount = 6, scale = 1.01 }: Props) {
  const x = useMotionValue(0.5);
  const y = useMotionValue(0.5);
  const sx = useSpring(x, { stiffness: 160, damping: 22, mass: 0.4 });
  const sy = useSpring(y, { stiffness: 160, damping: 22, mass: 0.4 });
  const rotateX = useTransform(sy, [0, 1], [amount, -amount]);
  const rotateY = useTransform(sx, [0, 1], [-amount, amount]);

  const onMove = (e: MouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    x.set((e.clientX - r.left) / r.width);
    y.set((e.clientY - r.top) / r.height);
  };
  const onLeave = () => {
    x.set(0.5);
    y.set(0.5);
  };

  return (
    <div style={{ perspective: 1200 }} className={cn("h-full", className)}>
      <motion.div
        onMouseMove={onMove}
        onMouseLeave={onLeave}
        style={{ rotateX, rotateY, transformStyle: "preserve-3d" }}
        whileHover={{ scale }}
        transition={{ type: "spring", stiffness: 200, damping: 24 }}
        className="h-full will-change-transform"
      >
        {children}
      </motion.div>
    </div>
  );
}
