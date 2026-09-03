import { pgTable, serial, text, timestamp, uniqueIndex } from "drizzle-orm/pg-core";

export const waitlistSignups = pgTable(
  "waitlist_signups",
  {
    id: serial("id").primaryKey(),
    email: text("email").notNull(),
    tier: text("tier").notNull(), // "pro" | "team"
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [uniqueIndex("waitlist_email_tier_idx").on(table.email, table.tier)],
);

export type WaitlistSignup = typeof waitlistSignups.$inferSelect;
