import { NextResponse } from "next/server";
import { db } from "@/db";
import { waitlistSignups } from "@/db/schema";

const TIERS = new Set(["pro", "team"]);
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export async function POST(req: Request) {
  let body: { email?: unknown; tier?: unknown };
  try {
    body = await req.json();
  } catch {
    return NextResponse.json({ ok: false, message: "Invalid request body." }, { status: 400 });
  }

  const email = typeof body.email === "string" ? body.email.trim().toLowerCase() : "";
  const tier = typeof body.tier === "string" ? body.tier : "";

  if (!EMAIL_RE.test(email) || email.length > 254) {
    return NextResponse.json({ ok: false, message: "Enter a valid email address." }, { status: 400 });
  }
  if (!TIERS.has(tier)) {
    return NextResponse.json({ ok: false, message: "Unknown tier." }, { status: 400 });
  }

  try {
    const inserted = await db
      .insert(waitlistSignups)
      .values({ email, tier })
      .onConflictDoNothing()
      .returning({ id: waitlistSignups.id });

    const label = tier === "pro" ? "Pro" : "Team";
    return NextResponse.json({
      ok: true,
      message: inserted.length > 0 ? `You are on the ${label} list.` : `Already on the ${label} list.`,
    });
  } catch (err) {
    console.error("waitlist insert failed", err);
    return NextResponse.json(
      { ok: false, message: "Could not save your email. Try again." },
      { status: 500 },
    );
  }
}
