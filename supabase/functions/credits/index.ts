// Supabase Edge Function: credits
// Endpoint:
//   GET /functions/v1/credits -> CreditStatus
//
// Requires `Authorization: Bearer <supabase_access_token>`.
// Settles 00:00 UTC daily allowance via `sync_user_daily_credits` and returns
// daily balance, wallet credit, paid_until window, active hourly pass, and checkout link.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
    },
  });
}

function safeHttpUrl(raw: unknown, fallback: string): string {
  const s = typeof raw === "string" ? raw.trim() : "";
  if (/^https?:\/\//i.test(s)) return s;
  return fallback;
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }
  if (req.method !== "GET") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  const authHeader = req.headers.get("Authorization") ?? "";
  const bearer = authHeader.startsWith("Bearer ") ? authHeader.slice(7).trim() : "";
  if (!bearer) {
    return jsonResponse({ error: "unauthorized", message: "Missing bearer token" }, 401);
  }

  const supabaseUrl = Deno.env.get("SUPABASE_URL") ?? "";
  const serviceRoleKey = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "";
  const admin = createClient(supabaseUrl, serviceRoleKey, {
    auth: { persistSession: false, autoRefreshToken: false },
  });

  const { data: userData, error: userErr } = await admin.auth.getUser(bearer);
  if (userErr || !userData?.user) {
    return jsonResponse({ error: "unauthorized", message: "Invalid or expired token" }, 401);
  }

  const userId = userData.user.id;
  const envPayUrl = safeHttpUrl(Deno.env.get("OPENMI_PAY_URL"), "");

  // Preferred path: atomic UTC daily-reset + active pass sync RPC.
  const { data: synced, error: rpcErr } = await admin.rpc("sync_user_daily_credits", {
    p_user_id: userId,
  });

  if (!rpcErr && synced && typeof synced === "object") {
    const s = synced as Record<string, unknown>;
    const paymentLink = safeHttpUrl(s.payment_link, DEFAULT_PAYMENT_LINK);
    return jsonResponse({
      plan: String(s.plan ?? "free"),
      is_paid: Boolean(s.is_paid),
      has_paid: Boolean(s.has_paid),
      paid_until: s.paid_until ?? null,
      balance: Number(s.balance ?? 0),
      wallet_credit: Number(s.wallet_credit ?? 0),
      daily_limit: Number(s.daily_limit ?? 6),
      next_reset_at: s.next_reset_at ?? null,
      active_model: s.active_model ?? null,
      pass_expires_at: s.pass_expires_at ?? null,
      payment_link: paymentLink,
      pay_url: envPayUrl || paymentLink,
      topup_paused: Boolean(s.topup_paused),
    });
  }

  // Fallback for pre-migration databases.
  const { data: existing, error: selectErr } = await admin
    .from("user_credits")
    .select("balance, plan")
    .eq("user_id", userId)
    .maybeSingle();

  if (selectErr) {
    return jsonResponse({ error: "db_error", message: selectErr.message }, 500);
  }

  if (existing) {
    return jsonResponse({
      balance: Number(existing.balance),
      plan: String(existing.plan),
      wallet_credit: 0,
      daily_limit: existing.plan === "paid" ? 15 : 6,
      is_paid: existing.plan === "paid",
      payment_link: DEFAULT_PAYMENT_LINK,
      pay_url: envPayUrl || DEFAULT_PAYMENT_LINK,
    });
  }

  return jsonResponse({
    balance: 6,
    wallet_credit: 0,
    daily_limit: 6,
    plan: "free",
    is_paid: false,
    payment_link: DEFAULT_PAYMENT_LINK,
    pay_url: envPayUrl || DEFAULT_PAYMENT_LINK,
  });
});
