import { ImageResponse } from "next/og";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { site } from "@/lib/site";

export const size = { width: 1200, height: 630 };
export const contentType = "image/png";
export const alt = `${site.name} — ${site.tagline}`;
export const dynamic = "force-static";

/**
 * The social card. twitter:card is summary_large_image, so this must exist or
 * every shared link renders as an empty box. Composed here rather than
 * committed as a PNG so the wordmark stays in the real display face.
 */
export default async function Image() {
  const [display, mono, plate] = await Promise.all([
    readFile(join(process.cwd(), "assets/fonts/InstrumentSerif-Regular.ttf")),
    readFile(join(process.cwd(), "assets/fonts/DepartureMono-Regular.otf")),
    readFile(join(process.cwd(), "public/space/carina-plate.jpg")),
  ]);

  const plateUri = `data:image/jpeg;base64,${plate.toString("base64")}`;

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "flex-end",
          background: "#050506",
          position: "relative",
        }}
      >
        <img
          src={plateUri}
          alt=""
          width={1200}
          height={630}
          style={{ position: "absolute", inset: 0, objectFit: "cover", opacity: 0.55 }}
        />
        <div
          style={{
            position: "absolute",
            inset: 0,
            background: "linear-gradient(to bottom, rgba(5,5,6,0.35), #050506 78%)",
          }}
        />

        <div
          style={{ display: "flex", flexDirection: "column", position: "relative", padding: 72 }}
        >
          <div
            style={{
              fontFamily: "Departure",
              fontSize: 22,
              letterSpacing: 3,
              color: "#8b8a86",
              textTransform: "uppercase",
            }}
          >
            Rust · OpenAI-compatible · MIT
          </div>
          <div
            style={{
              fontFamily: "Instrument",
              fontSize: 116,
              color: "#eceae7",
              letterSpacing: -2,
              marginTop: 26,
              lineHeight: 1,
              display: "flex",
            }}
          >
            <span>gray</span>
            <span style={{ color: "#d4a373" }}>.</span>
          </div>
          <div
            style={{
              fontFamily: "Instrument",
              fontSize: 44,
              color: "#8b8a86",
              marginTop: 14,
            }}
          >
            The agent that fits in one binary.
          </div>
        </div>
      </div>
    ),
    {
      ...size,
      fonts: [
        { name: "Instrument", data: display, style: "normal", weight: 400 },
        { name: "Departure", data: mono, style: "normal", weight: 400 },
      ],
    },
  );
}
