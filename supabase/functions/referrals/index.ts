// Openmi Edge Function: GET / POST /functions/v1/referrals
// GET:  Returns the authenticated user's invite code, stats, and referred-by status.
// POST: Applies an invite code ({ code }) for the authenticated user (pending until first Paid pass).

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const CORS_HEADERS: Record<string, string> = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
  "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
};

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...CORS_HEADERS },
  });
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }
  if (req.method !== "GET" && req.method !== "POST") {
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

  if (req.method === "POST") {
    let body: Record<string, unknown> = {};
    try {
      body = (await req.json()) as Record<string, unknown>;
    } catch (_) {
      return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
    }

    const code = String(body.code ?? "").trim().toUpperCase();
    const { data, error } = await admin.rpc("apply_referral_code", {
      p_invitee_id: userId,
      p_code: code,
    });

    if (error) {
      return jsonResponse({ error: "referral_failed", message: error.message }, 500);
    }

    const result = (data && typeof data === "object" ? data : {}) as Record<string, unknown>;
    if (result.status !== "ok") {
      return jsonResponse(
        {
          error: String(result.error ?? "referral_failed"),
          message: String(result.message ?? "Could not apply invite code."),
        },
        400,
      );
    }

    return jsonResponse(result, 200);
  }

  // GET: fetch/create personal code, settings, stats, and referred-by status
  const [codeRes, settingsRes, outgoingRes, incomingRes] = await Promise.all([
    admin.rpc("get_or_create_referral_code", { p_user_id: userId }),
    admin
      .from("app_settings")
      .select("referral_enabled, referral_inviter_coins, referral_invitee_coins")
      .eq("id", 1)
      .maybeSingle(),
    admin
      .from("referrals")
      .select("status, inviter_coins_granted")
      .eq("inviter_id", userId),
    admin
      .from("referrals")
      .select("code, status, invitee_coins_granted, created_at, rewarded_at")
      .eq("invitee_id", userId)
      .maybeSingle(),
  ]);

  if (codeRes.error) {
    return jsonResponse({ error: "referral_lookup_failed", message: codeRes.error.message }, 500);
  }

  const outgoing = Array.isArray(outgoingRes.data) ? outgoingRes.data : [];
  let pendingCount = 0;
  let rewardedCount = 0;
  let coinsEarned = 0;
  for (const row of outgoing) {
    if (row.status === "rewarded") {
      rewardedCount += 1;
      coinsEarned += Number(row.inviter_coins_granted ?? 0);
    } else {
      pendingCount += 1;
    }
  }

  const s = settingsRes.data;
  return jsonResponse(
    {
      code: String(codeRes.data ?? ""),
      referral_enabled: s ? s.referral_enabled !== false : true,
      inviter_coins: s ? Number(s.referral_inviter_coins ?? 3) : 3,
      invitee_coins: s ? Number(s.referral_invitee_coins ?? 3) : 3,
      stats: {
        total_invited: outgoing.length,
        pending_count: pendingCount,
        rewarded_count: rewardedCount,
        coins_earned: coinsEarned,
      },
      referred_by: incomingRes.data || null,
    },
    200,
  );
});
