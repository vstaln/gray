import * as TabsPrimitive from "@radix-ui/react-tabs";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";
import { cn } from "../../utils/cn";

export const Tabs = TabsPrimitive.Root;

export const TabsList = forwardRef<ElementRef<typeof TabsPrimitive.List>, ComponentPropsWithoutRef<typeof TabsPrimitive.List>>(
  ({ className, ...props }, ref) => (
    <TabsPrimitive.List
      ref={ref}
      className={cn("inline-flex h-10 items-center gap-1 rounded-sm border border-ink-800 bg-ink-900/70 p-1", className)}
      {...props}
    />
  ),
);
TabsList.displayName = "TabsList";

export const TabsTrigger = forwardRef<
  ElementRef<typeof TabsPrimitive.Trigger>,
  ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      "focus-ring mono relative inline-flex h-8 items-center justify-center rounded-xs px-3 text-[12px] uppercase tracking-[0.1em] text-ink-400 transition-colors duration-300 hover:text-ink-100 data-[state=active]:bg-ink-800 data-[state=active]:text-ink-50 data-[state=active]:shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]",
      className,
    )}
    {...props}
  />
));
TabsTrigger.displayName = "TabsTrigger";

export const TabsContent = forwardRef<
  ElementRef<typeof TabsPrimitive.Content>,
  ComponentPropsWithoutRef<typeof TabsPrimitive.Content>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Content ref={ref} className={cn("focus-ring mt-4 outline-none", className)} {...props} />
));
TabsContent.displayName = "TabsContent";
