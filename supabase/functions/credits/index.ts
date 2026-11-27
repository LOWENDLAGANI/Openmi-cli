// Supabase Edge Function: credits
// Endpoint:
//   GET /functions/v1/credits -> CreditStatus
//
// Requires `Authorization: Bearer <supabase_access_token>`.
// Settles 00:00 UTC daily allowance via `sync_user_daily_credits` and returns
// daily balance, wallet credit, paid_until window, active hourly pass, and checkout link.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { checkSimpleRateLimit } from "../_shared/upstash.ts";

const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";

function jsonResponse(body: unknown, status = 200, extraHeaders?: Record<string, string>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
      ...(extraHeaders ?? {}),
    },
  });
}

function safeHttpUrl(raw: unknown, fallback: string): string {
  const s = typeof raw === "string" ? raw.trim() : "";
  if (/^https?:\/\//i.test(s)) return s;
  return fallback;
}

function derivePayUrl(): string {
  const explicit = safeHttpUrl(Deno.env.get("OPENMI_PAY_URL"), "");
  if (explicit) return explicit;
  const verifyUrl = safeHttpUrl(Deno.env.get("OPENMI_CLI_VERIFY_URL"), "");
  if (verifyUrl) {
    try {
      return new URL("/pay", verifyUrl).toString();
    } catch (_) {
      // ignore invalid URL
    }
  }
  return "";
}

function deriveProviderUrl(): string {
  const explicit = safeHttpUrl(Deno.env.get("OPENMI_PROVIDER_URL"), "");
  if (explicit) return explicit;
  const verifyUrl = safeHttpUrl(Deno.env.get("OPENMI_CLI_VERIFY_URL"), "");
  if (verifyUrl) {
    try {
      return new URL("/provider", verifyUrl).toString();
    } catch (_) {
      // ignore invalid URL
    }
  }
  const payUrl = safeHttpUrl(Deno.env.get("OPENMI_PAY_URL"), "");
  if (payUrl) {
    try {
      return new URL("/provider", payUrl).toString();
    } catch (_) {
      // ignore invalid URL
    }
  }
  return "";
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
  const rl = await checkSimpleRateLimit("credits", userId, 60, 60);
  if (!rl.allowed) {
    return jsonResponse(
      { error: "rate_limited", message: "Too many credit status requests. Please wait a moment." },
      429,
      { "Retry-After": String(rl.resetSecs) },
    );
  }
  const envPayUrl = derivePayUrl();
  const envProviderUrl = deriveProviderUrl();

  // Preferred path: atomic UTC daily-reset + active pass sync RPC, plus live service status.
  const [{ data: synced, error: rpcErr }, { data: settingsRow }] = await Promise.all([
    admin.rpc("sync_user_daily_credits", { p_user_id: userId }),
    admin.from("app_settings").select("ai_enabled, pass_duration_minutes").eq("id", 1).maybeSingle(),
  ]);

  const aiEnabled = settingsRow ? settingsRow.ai_enabled !== false : true;
  const passDurationMinutes =
    settingsRow && typeof settingsRow.pass_duration_minutes === "number" && settingsRow.pass_duration_minutes > 0
      ? Math.floor(settingsRow.pass_duration_minutes)
      : 60;

  if (!rpcErr && synced && typeof synced === "object") {
    const s = synced as Record<string, unknown>;
    const isPaid = Boolean(s.is_paid);
    const paymentLink = safeHttpUrl(s.payment_link, DEFAULT_PAYMENT_LINK);
    return jsonResponse({
      plan: String(s.plan ?? "free"),
      is_paid: isPaid,
      has_paid: Boolean(s.has_paid),
      paid_until: s.paid_until ?? null,
      balance: Number(s.balance ?? 0),
      wallet_credit: Number(s.wallet_credit ?? 0),
      daily_limit: Number(s.daily_limit ?? (isPaid ? 15 : 6)),
      daily_tokens_used: Number(s.daily_tokens_used ?? 0),
      daily_token_limit: Number(s.daily_token_limit ?? (isPaid ? 7_000_000 : 2_000_000)),
      total_tokens_used: Number(s.total_tokens_used ?? 0),
      max_steps: Number(s.max_steps ?? (isPaid ? 70 : 40)),
      pass_duration_minutes: passDurationMinutes,
      ai_enabled: aiEnabled,
      next_reset_at: s.next_reset_at ?? null,
      active_model: s.active_model ?? null,
      pass_expires_at: s.pass_expires_at ?? null,
      payment_link: paymentLink,
      pay_url: envPayUrl || paymentLink,
      provider_url: envProviderUrl || null,
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
    const isPaid = existing.plan === "paid";
    return jsonResponse({
      balance: Number(existing.balance),
      plan: String(existing.plan),
      wallet_credit: 0,
      daily_limit: isPaid ? 15 : 6,
      daily_tokens_used: 0,
      daily_token_limit: isPaid ? 7_000_000 : 2_000_000,
      max_steps: isPaid ? 70 : 40,
      is_paid: isPaid,
      payment_link: DEFAULT_PAYMENT_LINK,
      pay_url: envPayUrl || DEFAULT_PAYMENT_LINK,
      provider_url: envProviderUrl || null,
    });
  }

  return jsonResponse({
    balance: 6,
    wallet_credit: 0,
    daily_limit: 6,
    daily_tokens_used: 0,
    daily_token_limit: 2_000_000,
    max_steps: 40,
    plan: "free",
    is_paid: false,
    payment_link: DEFAULT_PAYMENT_LINK,
    pay_url: envPayUrl || DEFAULT_PAYMENT_LINK,
    provider_url: envProviderUrl || null,
  });
});
