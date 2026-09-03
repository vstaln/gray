import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";
import { cn } from "../../utils/cn";

const badgeVariants = cva(
  "mono inline-flex items-center gap-1.5 rounded-xs border px-2 py-0.5 text-[10.5px] uppercase tracking-[0.14em]",
  {
    variants: {
      variant: {
        default: "border-ink-700 bg-ink-900/60 text-ink-300",
        accent: "border-accent/40 bg-accent/10 text-accent",
        outline: "border-ink-600 text-ink-200",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
