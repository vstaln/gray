import { useEffect, useRef, useState } from "react";
import { cn } from "../../utils/cn";

const CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!<>-_\\/[]{}—=+*^?#";

type Props = {
  text: string;
  className?: string;
  /** trigger: "hover" | "view" | "mount" */
  trigger?: "hover" | "view" | "mount";
  speed?: number;
  /** how many extra cycles per char before it settles */
  iterations?: number;
};

/**
 * ReactBits-style DecryptedText: scrambles characters then resolves left-to-right.
 */
export function DecryptedText({ text, className, trigger = "hover", speed = 28, iterations = 6 }: Props) {
  const [display, setDisplay] = useState(text);
  const [running, setRunning] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  const raf = useRef<number | null>(null);
  const started = useRef(false);

  const run = () => {
    if (running) return;
    setRunning(true);
    let frame = 0;
    const total = text.length * 2 + iterations;
    const tick = () => {
      frame += 1;
      const settled = Math.floor((frame / total) * text.length);
      const out = Array.from(text)
        .map((ch, i) => {
          if (ch === " ") return " ";
          if (i < settled) return ch;
          return CHARS[Math.floor(Math.random() * CHARS.length)];
        })
        .join("");
      setDisplay(out);
      if (frame < total) {
        raf.current = window.setTimeout(tick, speed) as unknown as number;
      } else {
        setDisplay(text);
        setRunning(false);
      }
    };
    tick();
  };

  useEffect(() => {
    if (trigger === "mount") run();
    if (trigger === "view" && ref.current) {
      const io = new IntersectionObserver(
        (entries) => {
          if (entries[0].isIntersecting && !started.current) {
            started.current = true;
            run();
            io.disconnect();
          }
        },
        { threshold: 0.6 },
      );
      io.observe(ref.current);
      return () => io.disconnect();
    }
    return () => {
      if (raf.current) clearTimeout(raf.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <span
      ref={ref}
      className={cn("inline-block", className)}
      onMouseEnter={trigger === "hover" ? run : undefined}
      aria-label={text}
    >
      {display}
    </span>
  );
}
