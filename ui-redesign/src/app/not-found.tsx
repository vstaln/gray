import Link from "next/link";

export default function NotFound() {
  return (
    <main className="mx-auto flex min-h-[100dvh] max-w-7xl flex-col justify-center px-5 sm:px-8">
      <p className="mono text-[12px] uppercase tracking-[0.18em] text-ink-400">404</p>
      <h1 className="display mt-4 text-[clamp(2.5rem,7vw,5rem)] font-semibold leading-[0.98] text-ink-50">
        Nothing at this path.
      </h1>
      <p className="mt-5 max-w-[42ch] text-[16px] leading-relaxed text-ink-300">
        The page may have moved, or the link was mistyped. The install command still works from
        the home page.
      </p>
      <Link
        href="/"
        className="focus-ring mt-8 inline-flex h-11 w-fit items-center rounded-sm bg-ink-50 px-5 text-[14px] font-medium text-ink-950 transition-colors hover:bg-white"
      >
        Back to gray
      </Link>
    </main>
  );
}
