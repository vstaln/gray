import { Toaster } from "sonner";
import { Features } from "./components/Features";
import { Footer } from "./components/Footer";
import { Hero } from "./components/Hero";
import { Install } from "./components/Install";
import { Loop } from "./components/Loop";
import { Manifesto } from "./components/Manifesto";
import { Nav } from "./components/Nav";
import { Pricing } from "./components/Pricing";
import { Providers } from "./components/Providers";
import { Stats } from "./components/Stats";
import { TooltipProvider } from "./components/ui/tooltip";
import { useLenis } from "./hooks/useLenis";

export default function App() {
  useLenis();

  return (
    <TooltipProvider delayDuration={200}>
      <div className="grain relative min-h-screen bg-ink-950 text-ink-100">
        <Nav />
        <main>
          <Hero />
          <Providers />
          <Features />
          <Manifesto />
          <Loop />
          <Stats />
          <Install />
          <Pricing />
        </main>
        <Footer />
      </div>
      <Toaster
        theme="dark"
        position="bottom-center"
        toastOptions={{
          classNames: {
            toast: "!bg-ink-900 !border-ink-700 !text-ink-100 !rounded-sm !shadow-2xl",
            description: "!text-ink-400 !font-mono !text-[11.5px]",
            title: "!text-[13px]",
          },
        }}
      />
    </TooltipProvider>
  );
}
