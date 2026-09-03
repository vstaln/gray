export function Footer() {
  return (
    <footer className="border-t border-ink-800">
      <div className="mx-auto flex max-w-7xl flex-col gap-8 px-5 py-14 sm:px-8 md:flex-row md:items-end md:justify-between">
        <div>
          <p className="display text-[28px] font-semibold leading-none text-ink-50">gray</p>
          <p className="mt-3 max-w-[38ch] text-[14px] leading-relaxed text-ink-400">
            A minimal, modular agent harness in Rust. MIT licensed, built by vstaln.
          </p>
        </div>
        <nav aria-label="Footer" className="flex flex-wrap gap-x-7 gap-y-3 text-[14px] text-ink-300">
          <a className="focus-ring rounded-xs transition-colors hover:text-ink-50" href="https://github.com/vstaln/gray" target="_blank" rel="noreferrer">
            GitHub
          </a>
          <a className="focus-ring rounded-xs transition-colors hover:text-ink-50" href="https://github.com/vstaln/gray/issues" target="_blank" rel="noreferrer">
            Issues
          </a>
          <a className="focus-ring rounded-xs transition-colors hover:text-ink-50" href="https://gray.alignment.id/dl" target="_blank" rel="noreferrer">
            Downloads
          </a>
          <a className="focus-ring rounded-xs transition-colors hover:text-ink-50" href="https://github.com/vstaln/gray/blob/main/LICENSE" target="_blank" rel="noreferrer">
            License
          </a>
        </nav>
      </div>
    </footer>
  );
}
