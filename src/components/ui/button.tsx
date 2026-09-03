import { cva, type VariantProps } from "class-variance-authority";
import { forwardRef, type ButtonHTMLAttributes } from "react";
import { cn } from "../../utils/cn";

const buttonVariants = cva(
  "focus-ring inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-sm text-[13.5px] font-medium transition-[background-color,color,border-color,transform,box-shadow] duration-300 ease-[cubic-bezier(0.16,1,0.3,1)] disabled:pointer-events-none disabled:opacity-50 active:scale-[0.98]",
  {
    variants: {
      variant: {
        default:
          "bg-ink-50 text-ink-950 hover:bg-white shadow-[0_0_0_1px_rgba(255,255,255,0.1),0_8px_24px_-12px_rgba(242,243,245,0.5)]",
        accent:
          "bg-accent text-ink-950 hover:bg-accent-strong shadow-[0_0_0_1px_rgba(217,176,97,0.4),0_8px_28px_-10px_rgba(217,176,97,0.55)]",
        outline:
          "border border-ink-700 bg-ink-900/40 text-ink-100 hover:border-ink-500 hover:bg-ink-800/60 backdrop-blur",
        ghost: "text-ink-300 hover:text-ink-50 hover:bg-ink-800/60",
        link: "text-ink-300 underline-offset-4 hover:text-accent hover:underline",
      },
      size: {
        default: "h-10 px-4",
        sm: "h-8 px-3 text-[12.5px]",
        lg: "h-12 px-6 text-[14.5px]",
        icon: "h-9 w-9",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(({ className, variant, size, ...props }, ref) => (
  <button ref={ref} className={cn(buttonVariants({ variant, size }), className)} {...props} />
));
Button.displayName = "Button";

export { buttonVariants };
