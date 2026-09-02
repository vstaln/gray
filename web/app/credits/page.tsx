import type { Metadata } from "next";
import { Footer } from "@/components/footer";
import { Nav } from "@/components/nav";

export const metadata: Metadata = {
  title: "Image credits",
  description: "Sources and licenses for every image and typeface used on this site.",
};

const images: Array<[string, string, string]> = [
  ["Carina Nebula — Cosmic Cliffs", "NASA · ESA · CSA · STScI (JWST NIRCam)", "hero, Pro tier"],
  ["Pillars of Creation", "NASA · ESA · Hubble Heritage / STScI", "footer"],
  ["Andromeda Galaxy", "NASA · JPL-Caltech", "Team tier"],
  ["Earth's limb at sunrise (STS-52)", "NASA Johnson Space Center", "install section"],
  ["Center of the Milky Way", "NASA · JPL-Caltech", "pricing section"],
  ["Blue Marble", "NASA Goddard Space Flight Center", "Free tier"],
  ["Apollo 16 Lunar Module", "NASA Johnson Space Center", "panel 01"],
  ["Jupiter's swirling storms", "NASA · JPL-Caltech · SwRI · MSSS (Juno)", "panel 02"],
  ["Helix Nebula (NGC 7293)", "NASA · JPL-Caltech", "panel 03"],
  ["Saturn backlit", "NASA · JPL-Caltech · Space Science Institute (Cassini)", "panel 04"],
  ["Aurora over North America", "NASA Goddard Space Flight Center", "panel 05"],
  ["2017 total solar eclipse", "NASA Armstrong Flight Research Center", "panel 06"],
];

const fonts: Array<[string, string, string]> = [
  ["Instrument Serif", "Rodrigo Fuenzalida / Instrument", "SIL OFL 1.1"],
  ["Newsreader", "Production Type", "SIL OFL 1.1"],
  ["Departure Mono", "Helena Zhang", "SIL OFL 1.1"],
];

export default function Credits() {
  return (
    <>
      <Nav />
      <main className="mx-auto max-w-[1200px] px-6 pb-24 pt-32">
        <p className="eyebrow">Colophon</p>
        <h1 className="display mt-4 text-[clamp(2.5rem,5vw,4rem)]">Image credits</h1>
        <p className="mt-5 max-w-[62ch] text-dim">
          Every photograph on this site comes from NASA&apos;s public-domain image library. Images
          are reprocessed — desaturated, contrast-lifted, or reduced to a one-bit Floyd–Steinberg
          dither — but not altered in content. NASA does not endorse gray.
        </p>

        <table className="mt-14 w-full border-collapse text-left">
          <thead>
            <tr className="border-b border-ink-700">
              <th className="eyebrow py-2 pr-6 font-normal">Image</th>
              <th className="eyebrow py-2 pr-6 font-normal">Credit</th>
              <th className="eyebrow py-2 font-normal">Used in</th>
            </tr>
          </thead>
          <tbody>
            {images.map(([title, credit, where]) => (
              <tr key={title} className="border-b border-ink-800">
                <td className="py-3 pr-6 align-top text-[15px] text-paper">{title}</td>
                <td className="py-3 pr-6 align-top text-[15px] text-dim">{credit}</td>
                <td className="py-3 align-top font-mono text-[11px] uppercase tracking-[0.08em] text-dimmer">
                  {where}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        <h2 className="display mt-20 text-[2rem]">Typefaces</h2>
        <table className="mt-6 w-full border-collapse text-left">
          <thead>
            <tr className="border-b border-ink-700">
              <th className="eyebrow py-2 pr-6 font-normal">Face</th>
              <th className="eyebrow py-2 pr-6 font-normal">Designer</th>
              <th className="eyebrow py-2 font-normal">License</th>
            </tr>
          </thead>
          <tbody>
            {fonts.map(([face, designer, license]) => (
              <tr key={face} className="border-b border-ink-800">
                <td className="py-3 pr-6 align-top text-[15px] text-paper">{face}</td>
                <td className="py-3 pr-6 align-top text-[15px] text-dim">{designer}</td>
                <td className="py-3 align-top font-mono text-[11px] uppercase tracking-[0.08em] text-sand-400">
                  {license}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </main>
      <Footer />
    </>
  );
}
