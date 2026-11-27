// Supabase Edge Function: cli-auth
// Endpoints:
//   POST /start    -> { device_code, user_code, verify_url, expires_in, interval }
//   POST /poll     -> { status, access_token?, refresh_token?, expires_at?, user? }
//   POST /approve  -> Approves or denies a user_code for the logged-in Supabase user
//   GET  /verify   -> Web approval page for interactive browser sign-in & code approval

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { checkSimpleRateLimit, getClientIp, isUpstashConfigured } from "../_shared/upstash.ts";

const DEVICE_CODE_TTL_SECONDS = 600; // 10 minutes
const POLL_INTERVAL_SECONDS = 5;
const MIN_POLL_GAP_MS = 2500;
const MAX_POLL_ATTEMPTS = 150;
const START_RATE_WINDOW_MS = 60_000;
const MAX_STARTS_PER_IP_PER_MINUTE = 10;

const startRateBuckets = new Map<string, number[]>();

function allowStartFromIp(ip: string): boolean {
  const now = Date.now();
  const cutoff = now - START_RATE_WINDOW_MS;
  const recent = (startRateBuckets.get(ip) ?? []).filter((ts) => ts > cutoff);
  if (recent.length >= MAX_STARTS_PER_IP_PER_MINUTE) {
    startRateBuckets.set(ip, recent);
    return false;
  }
  recent.push(now);
  startRateBuckets.set(ip, recent);
  // Prevent unbounded map growth across long-lived isolate instances.
  if (startRateBuckets.size > 2000) {
    for (const [k, list] of startRateBuckets) {
      if (list.every((ts) => ts <= cutoff)) startRateBuckets.delete(k);
    }
  }
  return true;
}

const USER_CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    },
  });
}

function randomHex(byteLen: number): string {
  const bytes = new Uint8Array(byteLen);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function generateUserCode(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  let out = "";
  for (let i = 0; i < 8; i++) {
    if (i === 4) out += "-";
    out += USER_CODE_ALPHABET[bytes[i] % USER_CODE_ALPHABET.length];
  }
  return out;
}

// Read the `exp` claim (unix seconds) from a JWT without verifying it; the token itself was
// already validated via `admin.auth.getUser`. Falls back to `fallbackSecs` from now.
function jwtExpiryIso(token: string, fallbackSecs: number): string {
  try {
    const payload = token.split(".")[1] ?? "";
    const json = JSON.parse(atob(payload.replace(/-/g, "+").replace(/_/g, "/")));
    if (typeof json.exp === "number" && json.exp > 0) {
      return new Date(json.exp * 1000).toISOString();
    }
  } catch (_) {
    // ignore malformed payloads
  }
  return new Date(Date.now() + fallbackSecs * 1000).toISOString();
}

async function sha256Hex(value: string): Promise<string> {
  const data = new TextEncoder().encode(value);
  const hash = await crypto.subtle.digest("SHA-256", data);
  return Array.from(new Uint8Array(hash), (b) => b.toString(16).padStart(2, "0")).join("");
}

function renderVerifyHtml(prefillCode: string, approveUrl: string): string {
  const safeCode = prefillCode.replace(/[^A-Za-z0-9-]/g, "").toUpperCase();
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>Authorize Openmi CLI</title>
  <style>
    body { font-family: system-ui, -apple-system, sans-serif; max-width: 440px; margin: 48px auto; padding: 0 16px; color: #111; }
    .card { border: 1px solid #ddd; border-radius: 10px; padding: 24px; }
    input, button { width: 100%; box-sizing: border-box; padding: 10px 12px; margin-top: 10px; font-size: 16px; }
    button { background: #111; color: #fff; border: 0; border-radius: 6px; cursor: pointer; }
    button.deny { background: #eee; color: #111; }
    #status { margin-top: 14px; font-size: 14px; }
  </style>
</head>
<body>
  <div class="card">
    <h2>Authorize Openmi CLI</h2>
    <p>Confirm the code shown in your terminal to sign in.</p>
    <label for="code">Device Code</label>
    <input id="code" value="${safeCode}" placeholder="ABCD-EFGH" autocomplete="off" />
    <label for="token">Supabase Access Token (from your signed-in Openmi session)</label>
    <input id="token" type="password" placeholder="Paste session access token" />
    <label for="refresh">Refresh Token (optional, keeps the CLI signed in past 1 hour)</label>
    <input id="refresh" type="password" placeholder="Paste session refresh token" />
    <button id="approve">Approve Device</button>
    <button id="deny" class="deny">Deny</button>
    <div id="status"></div>
  </div>
  <script>
    async function submitDecision(action) {
      const code = document.getElementById("code").value.trim().toUpperCase();
      const token = document.getElementById("token").value.trim();
      const refresh = document.getElementById("refresh").value.trim();
      const status = document.getElementById("status");
      status.textContent = "Submitting...";
      const res = await fetch(${JSON.stringify(approveUrl)}, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Authorization": "Bearer " + token
        },
        body: JSON.stringify(refresh ? { user_code: code, action, refresh_token: refresh } : { user_code: code, action })
      });
      const data = await res.json().catch(() => ({}));
      status.textContent = res.ok
        ? (action === "approve" ? "Approved! You can return to your terminal." : "Request denied.")
        : ("Error: " + (data.error || res.statusText));
    }
    document.getElementById("approve").onclick = () => submitDecision("approve");
    document.getElementById("deny").onclick = () => submitDecision("deny");
  </script>
</body>
</html>`;
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }

  const supabaseUrl = (Deno.env.get("SUPABASE_URL") ?? "").replace(/\/+$/, "");
  const serviceRoleKey = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "";
  const publicVerifyBase =
    (Deno.env.get("OPENMI_CLI_VERIFY_URL") ?? "").trim() ||
    `${supabaseUrl || new URL(req.url).origin.replace(/^http:\/\//, "https://")}/functions/v1/cli-auth/verify`;

  const admin = createClient(supabaseUrl, serviceRoleKey, {
    auth: { persistSession: false, autoRefreshToken: false },
  });

  const url = new URL(req.url);
  const path = url.pathname.replace(/\/+$/, "");

  // GET /verify?code=XXXX-XXXX
  if (req.method === "GET" && path.endsWith("/verify")) {
    const prefill = url.searchParams.get("code") ?? "";
    const approveUrl = `${url.origin}${path.replace(/\/verify$/, "/approve")}`;
    return new Response(renderVerifyHtml(prefill, approveUrl), {
      status: 200,
      headers: {
        "Content-Type": "text/html; charset=utf-8",
        "Cache-Control": "no-store",
        "X-Frame-Options": "DENY",
        "X-Content-Type-Options": "nosniff",
        "Referrer-Policy": "no-referrer",
        "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'",
      },
    });
  }

  // POST /start
  if (req.method === "POST" && path.endsWith("/start")) {
    // Rate-limit device code creation: distributed Upstash per-IP sliding window (with in-memory fallback) + global recent-pending cap.
    const clientIp = getClientIp(req);
    const startAllowed = isUpstashConfigured()
      ? (await checkSimpleRateLimit("cli_start", clientIp, MAX_STARTS_PER_IP_PER_MINUTE, 60)).allowed
      : allowStartFromIp(clientIp);
    if (!startAllowed) {
      return jsonResponse(
        { error: "rate_limited", message: "Too many login requests. Please wait a minute and try again." },
        429,
      );
    }

    const tenMinsAgo = new Date(Date.now() - DEVICE_CODE_TTL_SECONDS * 1000).toISOString();
    const { count: pendingCount } = await admin
      .from("device_codes")
      .select("device_code_hash", { count: "exact", head: true })
      .eq("status", "pending")
      .gte("created_at", tenMinsAgo);

    if ((pendingCount ?? 0) >= 500) {
      return jsonResponse(
        { error: "rate_limited", message: "Login service is temporarily busy. Please try again shortly." },
        429,
      );
    }

    const deviceCode = randomHex(32);
    const deviceCodeHash = await sha256Hex(deviceCode);
    const userCode = generateUserCode();
    const expiresAt = new Date(Date.now() + DEVICE_CODE_TTL_SECONDS * 1000).toISOString();

    const { error } = await admin.from("device_codes").insert({
      device_code_hash: deviceCodeHash,
      user_code: userCode,
      status: "pending",
      expires_at: expiresAt,
    });

    if (error) {
      return jsonResponse({ error: "failed_to_start", message: error.message }, 500);
    }

    const verifyUrl = `${publicVerifyBase}?code=${encodeURIComponent(userCode)}`;
    return jsonResponse({
      device_code: deviceCode,
      user_code: userCode,
      verify_url: verifyUrl,
      expires_in: DEVICE_CODE_TTL_SECONDS,
      interval: POLL_INTERVAL_SECONDS,
    });
  }

  // POST /approve (called by authenticated web user)
  if (req.method === "POST" && path.endsWith("/approve")) {
    const authHeader = req.headers.get("Authorization") ?? "";
    const bearer = authHeader.startsWith("Bearer ") ? authHeader.slice(7).trim() : "";
    if (!bearer) {
      return jsonResponse({ error: "unauthorized", message: "Missing bearer token" }, 401);
    }

    const { data: userData, error: userErr } = await admin.auth.getUser(bearer);
    if (userErr || !userData?.user) {
      return jsonResponse({ error: "unauthorized", message: "Invalid or expired user token" }, 401);
    }

    const approveRl = await checkSimpleRateLimit("cli_approve", userData.user.id, 15, 60);
    if (!approveRl.allowed) {
      return jsonResponse(
        { error: "rate_limited", message: "Too many approval attempts. Please wait a minute and try again." },
        429,
      );
    }

    const body = await req.json().catch(() => ({}));
    const userCode = String(body.user_code ?? "").trim().toUpperCase();
    const action = String(body.action ?? "approve").toLowerCase();
    const refreshToken = typeof body.refresh_token === "string" ? body.refresh_token : null;

    if (!userCode) {
      return jsonResponse({ error: "invalid_request", message: "user_code is required" }, 400);
    }

    const { data: row, error: fetchErr } = await admin
      .from("device_codes")
      .select("*")
      .eq("user_code", userCode)
      .maybeSingle();

    if (fetchErr || !row) {
      return jsonResponse({ error: "not_found", message: "Unknown device code" }, 404);
    }

    if (new Date(row.expires_at).getTime() <= Date.now()) {
      await admin
        .from("device_codes")
        .update({ status: "expired" })
        .eq("device_code_hash", row.device_code_hash);
      return jsonResponse({ error: "expired", message: "Device code expired" }, 410);
    }

    if (row.status !== "pending") {
      return jsonResponse({ error: "already_used", message: "Device code already processed" }, 409);
    }

    if (action === "deny") {
      await admin
        .from("device_codes")
        .update({ status: "denied" })
        .eq("device_code_hash", row.device_code_hash);
      return jsonResponse({ status: "denied" });
    }

    const tokenExpiresAt = jwtExpiryIso(bearer, 3600);
    const { data: approvedRow, error: updateErr } = await admin
      .from("device_codes")
      .update({
        status: "approved",
        user_id: userData.user.id,
        user_email: userData.user.email ?? null,
        access_token: bearer,
        refresh_token: refreshToken,
        token_expires_at: tokenExpiresAt,
      })
      .eq("device_code_hash", row.device_code_hash)
      .eq("status", "pending")
      .select("device_code_hash")
      .maybeSingle();

    if (updateErr) {
      return jsonResponse({ error: "update_failed", message: updateErr.message }, 500);
    }
    if (!approvedRow) {
      return jsonResponse({ error: "already_used", message: "Device code already processed" }, 409);
    }
    return jsonResponse({ status: "approved" });
  }

  // POST /poll
  if (req.method === "POST" && path.endsWith("/poll")) {
    const body = await req.json().catch(() => ({}));
    const deviceCode = String(body.device_code ?? "").trim();
    if (!deviceCode) {
      return jsonResponse({ error: "invalid_request", message: "device_code is required" }, 400);
    }

    const hash = await sha256Hex(deviceCode);
    const { data: row, error } = await admin
      .from("device_codes")
      .select("*")
      .eq("device_code_hash", hash)
      .maybeSingle();

    if (error || !row) {
      return jsonResponse({ status: "expired", error: "invalid_code" }, 404);
    }

    const nowMs = Date.now();
    if (new Date(row.expires_at).getTime() <= nowMs) {
      await admin
        .from("device_codes")
        .update({ status: "expired" })
        .eq("device_code_hash", hash);
      return jsonResponse({ status: "expired" }, 200);
    }

    // Rate-limit polling frequency and max attempts.
    if (row.poll_count >= MAX_POLL_ATTEMPTS) {
      return jsonResponse({ status: "expired", error: "rate_limit_exceeded" }, 429);
    }
    if (row.last_polled_at && nowMs - new Date(row.last_polled_at).getTime() < MIN_POLL_GAP_MS) {
      return jsonResponse({ status: "slow_down", interval: POLL_INTERVAL_SECONDS + 2 }, 429);
    }

    await admin
      .from("device_codes")
      .update({
        poll_count: row.poll_count + 1,
        last_polled_at: new Date(nowMs).toISOString(),
      })
      .eq("device_code_hash", hash);

    if (row.status === "pending") {
      return jsonResponse({ status: "pending", interval: POLL_INTERVAL_SECONDS });
    }
    if (row.status === "denied") {
      return jsonResponse({ status: "denied" });
    }
    if (row.status === "consumed") {
      return jsonResponse({ status: "expired", error: "already_consumed" }, 410);
    }

    if (row.status === "approved") {
      // Single-use: atomically consume and wipe stored tokens immediately on delivery.
      // Requiring a returned row prevents concurrent /poll requests from both receiving tokens.
      const { data: consumedRow, error: consumeErr } = await admin
        .from("device_codes")
        .update({
          status: "consumed",
          access_token: null,
          refresh_token: null,
        })
        .eq("device_code_hash", hash)
        .eq("status", "approved")
        .select("device_code_hash")
        .maybeSingle();

      if (consumeErr) {
        return jsonResponse({ error: "consume_failed" }, 500);
      }
      if (!consumedRow) {
        return jsonResponse({ status: "expired", error: "already_consumed" }, 410);
      }

      return jsonResponse({
        status: "approved",
        access_token: row.access_token,
        refresh_token: row.refresh_token ?? "",
        expires_at: Math.floor(new Date(row.token_expires_at ?? Date.now() + 3600_000).getTime() / 1000),
        user: {
          id: row.user_id,
          email: row.user_email,
        },
      });
    }

    return jsonResponse({ status: "expired" });
  }

  return jsonResponse({ error: "not_found" }, 404);
});
