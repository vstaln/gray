"use client";

import { ArrowRightIcon, CheckIcon } from "@phosphor-icons/react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type FormEvent } from "react";

type Status = "idle" | "loading" | "done" | "error";

export function WaitlistForm({ tier, emphasized = false }: { tier: "pro" | "team"; emphasized?: boolean }) {
  const [email, setEmail] = useState("");
  const [status, setStatus] = useState<Status>("idle");
  const [message, setMessage] = useState("");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (status === "loading") return;
    const trimmed = email.trim();
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed)) {
      setStatus("error");
      setMessage("Enter a valid email address.");
      return;
    }
    setStatus("loading");
    setMessage("");
    try {
      const res = await fetch("/api/waitlist", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: trimmed, tier }),
      });
      const data = (await res.json()) as { ok?: boolean; message?: string };
      if (!res.ok || !data.ok) {
        setStatus("error");
        setMessage(data.message ?? "Could not save your email. Try again.");
        return;
      }
      setStatus("done");
      setMessage(data.message ?? "You are on the list.");
    } catch {
      setStatus("error");
      setMessage("Connection failed. Try again.");
    }
  };

  const inputId = `waitlist-${tier}`;

  return (
    <div className="mt-auto pt-8">
      <AnimatePresence mode="wait" initial={false}>
        {status === "done" ? (
          <motion.div
            key="done"
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            className="flex h-11 items-center gap-2.5 rounded-sm border border-accent/40 bg-accent/10 px-3.5 text-[14px] text-ink-50"
            role="status"
          >
            <CheckIcon size={16} weight="bold" className="text-accent" />
            {message}
          </motion.div>
        ) : (
          <motion.form
            key="form"
            onSubmit={submit}
            noValidate
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0, y: -6 }}
          >
            <label htmlFor={inputId} className="mb-2 block text-[12.5px] text-ink-400">
              Get notified when {tier === "pro" ? "Pro" : "Team"} opens
            </label>
            <div
              className={`flex h-11 items-center rounded-sm border bg-ink-950 pl-3 pr-1 transition-colors duration-200 focus-within:border-accent ${
                status === "error" ? "border-red-400/70" : "border-ink-700"
              }`}
            >
              <input
                id={inputId}
                type="email"
                inputMode="email"
                autoComplete="email"
                required
                value={email}
                onChange={(e) => {
                  setEmail(e.target.value);
                  if (status === "error") setStatus("idle");
                }}
                placeholder="you@company.com"
                aria-invalid={status === "error"}
                aria-describedby={status === "error" ? `${inputId}-error` : undefined}
                className="min-w-0 flex-1 bg-transparent text-[14px] text-ink-50 outline-none placeholder:text-ink-500"
              />
              <motion.button
                type="submit"
                whileTap={{ scale: 0.96 }}
                disabled={status === "loading"}
                aria-label="Notify me"
                className={`focus-ring grid h-9 w-9 shrink-0 place-items-center rounded-xs transition-colors duration-200 disabled:opacity-60 ${
                  emphasized
                    ? "bg-accent text-ink-950 hover:bg-accent-strong"
                    : "bg-ink-50 text-ink-950 hover:bg-white"
                }`}
              >
                {status === "loading" ? (
                  <motion.span
                    animate={{ rotate: 360 }}
                    transition={{ repeat: Infinity, duration: 0.9, ease: "linear" }}
                    className="block h-3.5 w-3.5 rounded-full border-[1.5px] border-ink-950/30 border-t-ink-950"
                  />
                ) : (
                  <ArrowRightIcon size={16} weight="bold" />
                )}
              </motion.button>
            </div>
            <div className="h-5">
              {status === "error" ? (
                <p id={`${inputId}-error`} className="mt-1.5 text-[12.5px] text-red-300">
                  {message}
                </p>
              ) : null}
            </div>
          </motion.form>
        )}
      </AnimatePresence>
    </div>
  );
}
