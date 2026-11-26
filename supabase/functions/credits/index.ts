// Supabase Edge Function: credits
// Endpoint:
//   GET /functions/v1/credits -> { balance: number, plan: string }
//
// Requires `Authorization: Bearer <supabase_access_token>`.
// Auto-provisions a starter row (free plan, 100 credits) on first access.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const DEFAULT_STARTER_CREDITS = Number(Deno.env.get("OPENMI_STARTER_CREDITS") ?? "100");
const DEFAULT_PLAN = Deno.env.get("OPENMI_DEFAULT_PLAN") ?? "free";

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
    });
  }

  const starterBalance = Number.isFinite(DEFAULT_STARTER_CREDITS) ? DEFAULT_STARTER_CREDITS : 100;
  const { data: created, error: insertErr } = await admin
    .from("user_credits")
    .upsert(
      { user_id: userId, plan: DEFAULT_PLAN, balance: starterBalance },
      { onConflict: "user_id", ignoreDuplicates: true },
    )
    .select("balance, plan")
    .maybeSingle();

  if (insertErr) {
    return jsonResponse({ error: "db_error", message: insertErr.message }, 500);
  }

  return jsonResponse({
    balance: Number(created?.balance ?? starterBalance),
    plan: String(created?.plan ?? DEFAULT_PLAN),
  });
});
