// Supabase Edge Function: providers
// Endpoints (all require `Authorization: Bearer <supabase_access_token>`):
//   GET    /functions/v1/providers           -> list user's saved BYOK providers (keys masked)
//   POST   /functions/v1/providers           -> create a BYOK provider (encrypts API key via AES-256-GCM)
//   PATCH  /functions/v1/providers           -> update provider name, base_url, models, or rotate api_key
//   DELETE /functions/v1/providers?id=<uuid> -> delete a BYOK provider

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { checkSimpleRateLimit } from "../_shared/upstash.ts";
import {
  encryptApiKey,
  maskApiKey,
  normalizeByokModels,
  PROVIDER_PRESETS,
  validateProviderBaseUrl,
} from "../_shared/byok.ts";

const MAX_PROVIDERS_PER_USER = 10;

const CORS_HEADERS: Record<string, string> = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
  "Access-Control-Allow-Methods": "GET, POST, PATCH, DELETE, OPTIONS",
};

function jsonResponse(
  body: unknown,
  status = 200,
  extraHeaders?: Record<string, string>,
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      ...CORS_HEADERS,
      ...(extraHeaders ?? {}),
    },
  });
}

function sanitizeProviderRow(row: Record<string, unknown>) {
  return {
    id: row.id,
    name: row.name,
    preset: row.preset,
    base_url: row.base_url,
    api_key_hint: row.api_key_hint,
    models: Array.isArray(row.models) ? row.models : [],
    created_at: row.created_at,
    updated_at: row.updated_at,
  };
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
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

  const rl = await checkSimpleRateLimit("providers", userId, 60, 60);
  if (!rl.allowed) {
    return jsonResponse(
      { error: "rate_limited", message: "Too many provider requests. Please wait a moment." },
      429,
      { "Retry-After": String(rl.resetSecs) },
    );
  }

  if (req.method === "GET") {
    const { data, error } = await admin
      .from("user_providers")
      .select("id, name, preset, base_url, api_key_hint, models, created_at, updated_at")
      .eq("user_id", userId)
      .order("created_at", { ascending: true });

    if (error) {
      return jsonResponse({ error: "db_error", message: error.message }, 500);
    }

    return jsonResponse({
      providers: (data ?? []).map((r) => sanitizeProviderRow(r as Record<string, unknown>)),
      presets: PROVIDER_PRESETS,
    });
  }

  if (req.method === "POST") {
    let body: Record<string, unknown>;
    try {
      body = (await req.json()) as Record<string, unknown>;
    } catch (_) {
      return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
    }

    const { count, error: countErr } = await admin
      .from("user_providers")
      .select("id", { count: "exact", head: true })
      .eq("user_id", userId);
    if (countErr) {
      return jsonResponse({ error: "db_error", message: countErr.message }, 500);
    }
    if ((count ?? 0) >= MAX_PROVIDERS_PER_USER) {
      return jsonResponse(
        {
          error: "limit_reached",
          message: `You can save up to ${MAX_PROVIDERS_PER_USER} providers per account.`,
        },
        400,
      );
    }

    const presetRaw = typeof body.preset === "string" ? body.preset.trim().toLowerCase() : "custom";
    const preset = presetRaw in PROVIDER_PRESETS ? presetRaw : "custom";
    const name = (typeof body.name === "string" ? body.name.trim() : "").slice(0, 60) ||
      PROVIDER_PRESETS[preset].label;
    const rawBaseUrl =
      (typeof body.base_url === "string" && body.base_url.trim()) ||
      PROVIDER_PRESETS[preset].base_url;
    const urlCheck = validateProviderBaseUrl(rawBaseUrl);
    if (!urlCheck.ok) {
      return jsonResponse({ error: "invalid_base_url", message: urlCheck.error }, 400);
    }

    const rawApiKey = typeof body.api_key === "string" ? body.api_key.trim() : "";
    if (!rawApiKey) {
      return jsonResponse({ error: "invalid_api_key", message: "API key is required." }, 400);
    }
    if (rawApiKey.length > 512) {
      return jsonResponse({ error: "invalid_api_key", message: "API key is too long." }, 400);
    }

    const models = normalizeByokModels(body.models);
    if (models.length === 0) {
      return jsonResponse(
        { error: "invalid_models", message: "Add at least one model ID for this provider." },
        400,
      );
    }

    let apiKeyEnc: string;
    try {
      apiKeyEnc = await encryptApiKey(rawApiKey);
    } catch (err) {
      return jsonResponse(
        { error: "encryption_unavailable", message: err instanceof Error ? err.message : String(err) },
        503,
      );
    }

    const { data, error } = await admin
      .from("user_providers")
      .insert({
        user_id: userId,
        name,
        preset,
        base_url: urlCheck.url,
        api_key_enc: apiKeyEnc,
        api_key_hint: maskApiKey(rawApiKey),
        models,
      })
      .select("id, name, preset, base_url, api_key_hint, models, created_at, updated_at")
      .single();

    if (error) {
      return jsonResponse({ error: "db_error", message: error.message }, 500);
    }

    return jsonResponse({ provider: sanitizeProviderRow(data as Record<string, unknown>) }, 201);
  }

  if (req.method === "PATCH") {
    let body: Record<string, unknown>;
    try {
      body = (await req.json()) as Record<string, unknown>;
    } catch (_) {
      return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
    }

    const id = typeof body.id === "string" ? body.id.trim() : "";
    if (!id) {
      return jsonResponse({ error: "invalid_request", message: "Provider id is required." }, 400);
    }

    const updates: Record<string, unknown> = {
      updated_at: new Date().toISOString(),
    };

    if (typeof body.preset === "string") {
      const p = body.preset.trim().toLowerCase();
      updates.preset = p in PROVIDER_PRESETS ? p : "custom";
    }
    if (typeof body.name === "string" && body.name.trim()) {
      updates.name = body.name.trim().slice(0, 60);
    }
    if (typeof body.base_url === "string") {
      const urlCheck = validateProviderBaseUrl(body.base_url);
      if (!urlCheck.ok) {
        return jsonResponse({ error: "invalid_base_url", message: urlCheck.error }, 400);
      }
      updates.base_url = urlCheck.url;
    }
    if (Array.isArray(body.models)) {
      const models = normalizeByokModels(body.models);
      if (models.length === 0) {
        return jsonResponse(
          { error: "invalid_models", message: "Keep at least one model ID for this provider." },
          400,
        );
      }
      updates.models = models;
    }
    if (typeof body.api_key === "string" && body.api_key.trim()) {
      const rawKey = body.api_key.trim();
      if (rawKey.length > 512) {
        return jsonResponse({ error: "invalid_api_key", message: "API key is too long." }, 400);
      }
      try {
        updates.api_key_enc = await encryptApiKey(rawKey);
        updates.api_key_hint = maskApiKey(rawKey);
      } catch (err) {
        return jsonResponse(
          { error: "encryption_unavailable", message: err instanceof Error ? err.message : String(err) },
          503,
        );
      }
    }

    const { data, error } = await admin
      .from("user_providers")
      .update(updates)
      .eq("id", id)
      .eq("user_id", userId)
      .select("id, name, preset, base_url, api_key_hint, models, created_at, updated_at")
      .maybeSingle();

    if (error) {
      return jsonResponse({ error: "db_error", message: error.message }, 500);
    }
    if (!data) {
      return jsonResponse({ error: "not_found", message: "Provider not found." }, 404);
    }

    return jsonResponse({ provider: sanitizeProviderRow(data as Record<string, unknown>) }, 200);
  }

  if (req.method === "DELETE") {
    const url = new URL(req.url);
    const id = (url.searchParams.get("id") ?? "").trim();
    if (!id) {
      return jsonResponse({ error: "invalid_request", message: "Missing ?id=<provider_id>" }, 400);
    }

    const { error } = await admin
      .from("user_providers")
      .delete()
      .eq("id", id)
      .eq("user_id", userId);

    if (error) {
      return jsonResponse({ error: "db_error", message: error.message }, 500);
    }

    return jsonResponse({ ok: true, id }, 200);
  }

  return jsonResponse({ error: "method_not_allowed" }, 405);
});
