import type { Metadata } from "next";
import Link from "next/link";
import { Footer } from "@/components/footer";
import { Nav } from "@/components/nav";
import { Prose } from "@/components/prose";
import { site } from "@/lib/site";

export const metadata: Metadata = {
  title: "Terms",
  description: "Terms for the gray software and this website.",
};

export default function Terms() {
  return (
    <>
      <Nav />
      <main className="mx-auto max-w-[1200px] px-6 pb-24 pt-32">
        <Prose eyebrow="Legal" title="Terms">
          <h2>The software</h2>
          <p>
            gray is released under the MIT license. You may use, copy, modify and distribute it,
            including commercially, subject to that license. The full text ships with the source.
          </p>
          <p>
            It is provided as is, without warranty of any kind. gray executes shell commands and
            edits files on your machine at a model&apos;s direction — you are responsible for what
            you let it run and for the state of anything it touches.
          </p>

          <h2>Model providers</h2>
          <p>
            gray connects to model providers using credentials you supply. Your use of any provider
            is governed by that provider&apos;s own terms, and their charges are between you and
            them.
          </p>

          <h2>This site</h2>
          <p>
            Documentation and copy on this site are provided for reference and may change. NASA
            imagery is public domain; the typefaces are SIL OFL 1.1. See{" "}
            <Link href="/credits" className="text-sand-300 underline underline-offset-4">
              image credits
            </Link>
            .
          </p>

          <h2>Paid services</h2>
          <p>
            Paid tiers are not yet available. When they are, this page will describe billing,
            cancellation and refunds before any charge can be made.
          </p>

          <h2>Contact</h2>
          <p>
            <a
              href={site.repo}
              target="_blank"
              rel="noreferrer"
              className="text-sand-300 underline underline-offset-4"
            >
              {site.repo.replace("https://", "")}
            </a>
          </p>
        </Prose>
      </main>
      <Footer />
    </>
  );
}
