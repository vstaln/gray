import localFont from "next/font/local";

export const instrument = localFont({
  src: [
    { path: "../public/fonts/InstrumentSerif-Regular.woff2", weight: "400", style: "normal" },
    { path: "../public/fonts/InstrumentSerif-Italic.woff2", weight: "400", style: "italic" },
  ],
  variable: "--font-instrument",
  display: "swap",
  preload: true,
  fallback: ["Times New Roman", "serif"],
});

export const newsreader = localFont({
  src: "../public/fonts/Newsreader-Variable.woff2",
  weight: "200 700",
  variable: "--font-newsreader",
  display: "swap",
  preload: true,
  fallback: ["Georgia", "serif"],
});

export const departure = localFont({
  src: "../public/fonts/DepartureMono-Regular.woff2",
  weight: "400",
  variable: "--font-departure",
  display: "swap",
  preload: true,
  fallback: ["ui-monospace", "monospace"],
});
