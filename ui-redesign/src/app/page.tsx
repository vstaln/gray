import { Features } from "@/components/features";
import { Footer } from "@/components/footer";
import { Hero } from "@/components/hero";
import { Install } from "@/components/install";
import { Loop } from "@/components/loop";
import { Nav } from "@/components/nav";
import { Pricing } from "@/components/pricing";

export default function HomePage() {
  return (
    <>
      <Nav />
      <main id="main">
        <Hero />
        <Features />
        <Loop />
        <Install />
        <Pricing />
      </main>
      <Footer />
    </>
  );
}
