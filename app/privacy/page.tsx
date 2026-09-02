import type { Metadata } from "next";
import { Footer } from "@/components/footer";
import { Nav } from "@/components/nav";
import { Prose } from "@/components/prose";

export const metadata: Metadata = {
  title: "Privacy",
  description: "What gray collects, which is almost nothing.",
};

export default function Privacy() {
  return (
    <>
      <Nav />
      <main className="mx-auto max-w-[1200px] px-6 pb-24 pt-32">
        <Prose eyebrow="Legal" title="Privacy">
          <h2>The binary</h2>
          <p>
            gray is a local program. Conversations, sessions and credentials stay on your machine
            under <code>~/.gray</code>. Nothing is sent to us. When you talk to a model, your
            messages go directly from your machine to whichever provider you configured, under that
            provider&apos;s own privacy policy.
          </p>
          <p>
            The only network request gray makes on its own behalf is a version check against{" "}
            <code>gray.alignment.id/dl/latest-&lt;channel&gt;.txt</code>. It sends no identifiers.
          </p>

          <h2>This site</h2>
          <p>
            The site is static. It sets no cookies, runs no advertising or tracking scripts, and
            embeds no third-party fonts or analytics — every asset is served from this domain.
            Standard web-server access logs record IP addresses for a short period for abuse
            handling.
          </p>

          <h2>Paid accounts</h2>
          <p>
            If and when paid tiers ship, billing will run through a merchant of record. We would
            store your email, subscription state and usage records; card data would never reach our
            servers. This page will be updated before that happens.
          </p>

          <h2>Contact</h2>
          <p>
            Open an issue on the repository for anything about this page.
          </p>
        </Prose>
      </main>
      <Footer />
    </>
  );
}
