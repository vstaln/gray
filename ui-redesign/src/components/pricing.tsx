import { CheckIcon } from "@phosphor-icons/react/dist/ssr";
import { Reveal, RevealItem } from "./reveal";
import { WaitlistForm } from "./waitlist-form";

const tiers = [
  {
    id: "free",
    name: "Free",
    price: "$0",
    cadence: "forever",
    features: ["The full binary, MIT", "Every provider, bring your own keys", "All tools and skills", "Local sessions", "Community support"],
  },
  {
    id: "pro",
    name: "Pro",
    price: "$20",
    cadence: "per month",
    features: ["Hosted gateway", "Telegram, Discord and Slack", "Cloud cron", "Session sync", "Priority builds"],
  },
  {
    id: "team",
    name: "Team",
    price: "$100",
    cadence: "per month",
    features: ["Everything in Pro", "Five seats", "Shared skills registry", "Audit log", "SSO"],
  },
] as const;

export function Pricing() {
  return (
    <section id="pricing" className="relative scroll-mt-24 border-t border-ink-800">
      <div
        className="pointer-events-none absolute inset-0"
        aria-hidden
        style={{
          background:
            "radial-gradient(60% 50% at 50% 0%, rgba(217,176,97,0.07), transparent 70%)",
        }}
      />
      <div className="relative mx-auto max-w-7xl px-5 py-28 sm:px-8 sm:py-36">
        <Reveal className="max-w-2xl">
          <RevealItem as="h2" className="display text-[clamp(2rem,4.5vw,3.5rem)] font-semibold leading-[1.02] text-ink-50">
            Free forever. Paid only for what we host.
          </RevealItem>
          <RevealItem as="p" className="prose-tight mt-4 max-w-[54ch] text-[16px] leading-relaxed text-ink-300">
            The binary is MIT: every provider, every tool, your own keys. Paid tiers cover the
            gateway and cron we run for you, and are not open for signup yet.
          </RevealItem>
        </Reveal>

        <Reveal amount={0.15} className="mt-14 grid grid-cols-1 gap-3 lg:grid-cols-3">
          {tiers.map((t) => {
            const pro = t.id === "pro";
            return (
              <RevealItem
                key={t.id}
                className={`flex flex-col rounded-md p-7 sm:p-8 ${
                  pro
                    ? "border border-accent/40 bg-ink-900 shadow-[0_30px_80px_-30px_rgba(217,176,97,0.25)]"
                    : "border border-ink-800 bg-ink-900/50"
                }`}
              >
                <div className="flex h-7 items-center justify-between">
                  <h3 className="text-[15px] font-medium text-ink-50">{t.name}</h3>
                  {pro ? (
                    <span className="mono rounded-xs bg-accent px-1.5 py-0.5 text-[11px] font-medium uppercase tracking-wide text-ink-950">
                      Popular
                    </span>
                  ) : null}
                </div>

                <div className="mt-5 flex h-14 items-baseline gap-2">
                  <span className="display text-[44px] font-semibold leading-none text-ink-50">{t.price}</span>
                  <span className="text-[14px] text-ink-400">{t.cadence}</span>
                </div>

                <ul className="mt-8 flex flex-col gap-3">
                  {t.features.map((f) => (
                    <li key={f} className="flex items-start gap-3 text-[14.5px] text-ink-200">
                      <CheckIcon size={15} weight="bold" className={`mt-[3px] shrink-0 ${pro ? "text-accent" : "text-ink-400"}`} />
                      {f}
                    </li>
                  ))}
                </ul>

                {t.id === "free" ? (
                  <div className="mt-auto pt-8">
                    <a
                      href="#install"
                      className="focus-ring flex h-11 items-center justify-center rounded-sm bg-ink-50 text-[14px] font-medium text-ink-950 transition-all duration-200 hover:bg-white active:scale-[0.99]"
                    >
                      Install
                    </a>
                    <div className="h-5" />
                  </div>
                ) : (
                  <WaitlistForm tier={t.id} emphasized={pro} />
                )}
              </RevealItem>
            );
          })}
        </Reveal>
      </div>
    </section>
  );
}
