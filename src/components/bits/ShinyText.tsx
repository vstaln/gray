import { cn } from "../../utils/cn";

export function ShinyText({ text, className }: { text: string; className?: string }) {
  return <span className={cn("shiny", className)}>{text}</span>;
}
