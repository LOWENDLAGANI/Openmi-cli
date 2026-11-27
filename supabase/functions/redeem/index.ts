// Openmi Edge Function: POST /functions/v1/redeem
// Redeems a Coin giveaway / promo code for the authenticated user.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const CORS_HEADERS: Record<string, string> = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
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
  if (req.method !== "POST") {
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

  let body: Record<string, unknown> = {};
  try {
    body = (await req.json()) as Record<string, unknown>;
  } catch (_) {
    return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
  }

  const code = String(body.code ?? "").trim().toUpperCase();
  if (!code || code.length > 64) {
    return jsonResponse({ error: "invalid_code", message: "Please enter a valid redeem code." }, 400);
  }

  const { data, error } = await admin.rpc("redeem_coin_code", {
    p_user_id: userData.user.id,
    p_code: code,
  });

  if (error) {
    return jsonResponse({ error: "redeem_failed", message: error.message }, 500);
  }

  const result = (data && typeof data === "object" ? data : {}) as Record<string, unknown>;
  if (result.status !== "ok") {
    return jsonResponse(
      {
        error: String(result.error ?? "redeem_failed"),
        message: String(result.message ?? "Could not redeem code."),
      },
      400,
    );
  }

  return jsonResponse(result, 200);
});
