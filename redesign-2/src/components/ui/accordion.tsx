import * as AccordionPrimitive from "@radix-ui/react-accordion";
import { Plus } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";
import { cn } from "../../utils/cn";

export const Accordion = AccordionPrimitive.Root;

export const AccordionItem = forwardRef<
  ElementRef<typeof AccordionPrimitive.Item>,
  ComponentPropsWithoutRef<typeof AccordionPrimitive.Item>
>(({ className, ...props }, ref) => (
  <AccordionPrimitive.Item ref={ref} className={cn("border-b border-ink-800", className)} {...props} />
));
AccordionItem.displayName = "AccordionItem";

export const AccordionTrigger = forwardRef<
  ElementRef<typeof AccordionPrimitive.Trigger>,
  ComponentPropsWithoutRef<typeof AccordionPrimitive.Trigger>
>(({ className, children, ...props }, ref) => (
  <AccordionPrimitive.Header className="flex">
    <AccordionPrimitive.Trigger
      ref={ref}
      className={cn(
        "focus-ring group flex flex-1 items-center justify-between gap-4 py-4 text-left transition-colors hover:text-ink-50 [&[data-state=open]>svg]:rotate-45",
        className,
      )}
      {...props}
    >
      {children}
      <Plus className="h-4 w-4 shrink-0 text-ink-500 transition-transform duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] group-hover:text-accent" />
    </AccordionPrimitive.Trigger>
  </AccordionPrimitive.Header>
));
AccordionTrigger.displayName = "AccordionTrigger";

export const AccordionContent = forwardRef<
  ElementRef<typeof AccordionPrimitive.Content>,
  ComponentPropsWithoutRef<typeof AccordionPrimitive.Content>
>(({ className, children, ...props }, ref) => (
  <AccordionPrimitive.Content
    ref={ref}
    className="overflow-hidden text-[14px] text-ink-300 data-[state=closed]:animate-[acc-up_320ms_cubic-bezier(0.16,1,0.3,1)] data-[state=open]:animate-[acc-down_380ms_cubic-bezier(0.16,1,0.3,1)]"
    {...props}
  >
    <div className={cn("pb-4 pt-0", className)}>{children}</div>
  </AccordionPrimitive.Content>
));
AccordionContent.displayName = "AccordionContent";
