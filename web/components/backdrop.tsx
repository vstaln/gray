import { cn } from "@/lib/cn";

type Props = {
  /** Baked plate under the diffusion layer. */
  src: string;
  /** How much of the plate survives. Hero wants little; bands want less. */
  opacity?: number;
  /** Blur radius in px. The blur should eat the shape, not decorate it. */
  blur?: number;
  /** Fade direction of the mask into pure black. */
  fade?: "bottom" | "top" | "both" | "none";
  /** Slow parallax-free drift. Disabled under prefers-reduced-motion. */
  drift?: boolean;
  className?: string;
};

/**
 * form -> diffusion -> matter. Every background on the site is these three
 * layers and nothing else: a baked NASA plate, a heavy blur masked into the
 * page black, and a noise tile on top so no gradient reads as CGI.
 */
export function Backdrop({
  src,
  opacity = 0.5,
  blur = 0,
  fade = "bottom",
  drift = false,
  className,
}: Props) {
  const mask =
    fade === "bottom"
      ? "linear-gradient(to bottom, black 0%, black 38%, transparent 92%)"
      : fade === "top"
        ? "linear-gradient(to top, black 0%, black 38%, transparent 92%)"
        : fade === "both"
          ? "linear-gradient(to bottom, transparent 0%, black 26%, black 74%, transparent 100%)"
          : undefined;

  return (
    <div
      aria-hidden
      className={cn("pointer-events-none absolute inset-0 overflow-hidden grain", className)}
    >
      <div
        className={cn(
          "absolute inset-0 bg-cover bg-center",
          drift && "motion-safe:animate-[drift_38s_ease-in-out_infinite_alternate]",
        )}
        style={{
          backgroundImage: `url(${src})`,
          opacity,
          filter: blur ? `blur(${blur}px)` : undefined,
          maskImage: mask,
          WebkitMaskImage: mask,
          transform: blur ? "scale(1.08)" : undefined,
        }}
      />
      <div className="absolute inset-0 bg-gradient-to-b from-ink-950/40 via-transparent to-ink-950" />
    </div>
  );
}
