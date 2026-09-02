export function Prose({
  eyebrow,
  title,
  children,
}: {
  eyebrow: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <>
      <p className="eyebrow">{eyebrow}</p>
      <h1 className="display mt-4 text-[clamp(2.5rem,5vw,4rem)]">{title}</h1>
      <div
        className="mt-12 max-w-[70ch] space-y-5 border-t border-ink-700 pt-10 text-dim
          [&_code]:border [&_code]:border-ink-700 [&_code]:bg-ink-900 [&_code]:px-1.5
          [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[12px] [&_code]:text-sand-300
          [&_h2]:pt-6 [&_h2]:font-display [&_h2]:text-[1.75rem] [&_h2]:leading-tight
          [&_h2]:tracking-[-0.02em] [&_h2]:text-paper
          [&_li]:leading-relaxed [&_p]:leading-relaxed
          [&_ul]:list-none [&_ul]:space-y-2 [&_ul]:pl-0
          [&_ul_li]:before:mr-3 [&_ul_li]:before:text-ink-500 [&_ul_li]:before:content-['·']"
      >
        {children}
      </div>
    </>
  );
}
