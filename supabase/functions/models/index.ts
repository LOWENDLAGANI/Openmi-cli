// Supabase Edge Function: models
// Endpoint:
//   GET /functions/v1/models -> ModelInfo[] ({ id, label, context_window, credit_cost, paid_only })
//
// Reads live catalog from `public.app_settings.models` (if present), falling back to
// `OPENMI_MODELS_JSON` secret or the built-in default catalog.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import { checkSimpleRateLimit, getClientIp } from "../_shared/upstash.ts";

interface ModelInfo {
  id: string;
  label: string;
  context_window: number;
  credit_cost: number;
  paid_only: boolean;
  display_name?: string;
  description?: string;
}

const DEFAULT_MODELS: ModelInfo[] = [
  { id: "openmi-default", label: "Openmi Default", context_window: 128000, credit_cost: 3, paid_only: false },
  { id: "openai/gpt-4o-mini", label: "GPT-4o Mini", context_window: 128000, credit_cost: 2, paid_only: false },
  { id: "openai/gpt-4o", label: "GPT-4o", context_window: 128000, credit_cost: 3, paid_only: false },
  { id: "anthropic/claude-3.5-sonnet", label: "Claude 3.5 Sonnet", context_window: 200000, credit_cost: 3, paid_only: true },
];

function jsonResponse(body: unknown, status = 200, extraHeaders?: Record<string, string>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "public, max-age=15",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
      ...(extraHeaders ?? {}),
    },
  });
}

function normalizeModelList(value: unknown): ModelInfo[] | null {
  if (!Array.isArray(value) || value.length === 0) return null;
  const out: ModelInfo[] = [];
  for (const raw of value) {
    if (!raw || typeof raw !== "object") continue;
    const m = raw as Record<string, unknown>;
    const id = typeof m.id === "string" ? m.id.trim() : "";
    if (!id) continue;
    const label = typeof m.label === "string" && m.label.trim() ? m.label.trim() : id;
    const contextWindow =
      typeof m.context_window === "number" && Number.isFinite(m.context_window)
        ? Math.max(1000, Math.floor(m.context_window))
        : 128000;
    const creditCost =
      typeof m.credit_cost === "number" && Number.isFinite(m.credit_cost) && m.credit_cost >= 0
        ? Math.floor(m.credit_cost)
        : 3;
    const paidOnly = Boolean(m.paid_only);
    // NOTE: only public fields are copied. `api_key` / `base_url` are intentionally dropped
    // because this endpoint is readable by every client.
    const displayName = typeof m.display_name === "string" ? m.display_name.trim() : "";
    const description = typeof m.description === "string" ? m.description.trim() : "";
    out.push({
      id,
      label,
      context_window: contextWindow,
      credit_cost: creditCost,
      paid_only: paidOnly,
      ...(displayName ? { display_name: displayName } : {}),
      ...(description ? { description } : {}),
    });
  }
  return out.length > 0 ? out : null;
}

function loadEnvModels(): ModelInfo[] {
  const raw = Deno.env.get("OPENMI_MODELS_JSON");
  if (!raw) return DEFAULT_MODELS;
  try {
    const parsed = JSON.parse(raw);
    const normalized = normalizeModelList(parsed);
    if (normalized) return normalized;
  } catch (_) {
    // Ignore malformed OPENMI_MODELS_JSON.
  }
  return DEFAULT_MODELS;
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }
  if (req.method !== "GET") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  const clientIp = getClientIp(req);
  if (clientIp !== "unknown") {
    const rl = await checkSimpleRateLimit("models", clientIp, 60, 60);
    if (!rl.allowed) {
      return jsonResponse(
        { error: "rate_limited", message: "Too many model catalog requests. Please wait a moment." },
        429,
        { "Retry-After": String(rl.resetSecs) },
      );
    }
  }

  const supabaseUrl = Deno.env.get("SUPABASE_URL") ?? "";
  const serviceRoleKey = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "";
  if (supabaseUrl && serviceRoleKey) {
    try {
      const admin = createClient(supabaseUrl, serviceRoleKey, {
        auth: { persistSession: false, autoRefreshToken: false },
      });
      const { data } = await admin
        .from("app_settings")
        .select("models")
        .eq("id", 1)
        .maybeSingle();
      const normalized = normalizeModelList(data?.models);
      if (normalized) {
        return jsonResponse(normalized, 200);
      }
    } catch (_) {
      // Fall back to env/default if app_settings is not migrated yet.
    }
  }

  return jsonResponse(loadEnvModels(), 200);
});
