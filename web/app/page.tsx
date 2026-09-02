import { Footer } from "@/components/footer";
import { Hero } from "@/components/hero";
import { Install } from "@/components/install";
import { Nav } from "@/components/nav";
import { Panels } from "@/components/panels";
import { Pricing } from "@/components/pricing";
import { Proof } from "@/components/proof";

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <Panels />
        <Proof />
        <Install />
        <Pricing />
      </main>
      <Footer />
    </>
  );
}
