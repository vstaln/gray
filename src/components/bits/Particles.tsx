import { useEffect, useRef } from "react";
import { cn } from "../../utils/cn";

type Props = {
  className?: string;
  count?: number;
  /** cursor repulsion strength */
  repel?: number;
};

/**
 * ReactBits-style Particles: a sparse starfield on canvas that drifts and
 * gently reacts to the cursor. Kept deliberately quiet; this is set dressing.
 */
export function Particles({ className, count = 110, repel = 60 }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    let w = 0;
    let h = 0;
    let raf = 0;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const mouse = { x: -9999, y: -9999 };

    type P = { x: number; y: number; vx: number; vy: number; r: number; a: number; tw: number };
    let pts: P[] = [];

    const resize = () => {
      const r = canvas.getBoundingClientRect();
      w = r.width;
      h = r.height;
      canvas.width = w * dpr;
      canvas.height = h * dpr;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      pts = Array.from({ length: count }, () => ({
        x: Math.random() * w,
        y: Math.random() * h,
        vx: (Math.random() - 0.5) * 0.08,
        vy: (Math.random() - 0.5) * 0.08,
        r: Math.random() * 1.1 + 0.3,
        a: Math.random() * 0.5 + 0.15,
        tw: Math.random() * Math.PI * 2,
      }));
    };

    const draw = (t: number) => {
      ctx.clearRect(0, 0, w, h);
      for (const p of pts) {
        if (!reduce) {
          p.x += p.vx;
          p.y += p.vy;
          const dx = p.x - mouse.x;
          const dy = p.y - mouse.y;
          const d2 = dx * dx + dy * dy;
          if (d2 < repel * repel) {
            const d = Math.sqrt(d2) || 1;
            const f = (repel - d) / repel;
            p.x += (dx / d) * f * 1.2;
            p.y += (dy / d) * f * 1.2;
          }
          if (p.x < -5) p.x = w + 5;
          if (p.x > w + 5) p.x = -5;
          if (p.y < -5) p.y = h + 5;
          if (p.y > h + 5) p.y = -5;
        }
        const twinkle = 0.75 + 0.25 * Math.sin(t / 900 + p.tw);
        ctx.beginPath();
        ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
        // a few particles catch the ochre accent
        const accent = p.tw % 7 < 0.6;
        ctx.fillStyle = accent
          ? `rgba(217,176,97,${p.a * twinkle})`
          : `rgba(221,224,229,${p.a * twinkle})`;
        ctx.fill();
      }
      raf = requestAnimationFrame(draw);
    };

    const onMove = (e: MouseEvent) => {
      const r = canvas.getBoundingClientRect();
      mouse.x = e.clientX - r.left;
      mouse.y = e.clientY - r.top;
    };
    const onLeave = () => {
      mouse.x = -9999;
      mouse.y = -9999;
    };

    resize();
    raf = requestAnimationFrame(draw);
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseleave", onLeave);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseleave", onLeave);
    };
  }, [count, repel]);

  return <canvas ref={ref} className={cn("pointer-events-none h-full w-full", className)} aria-hidden />;
}
