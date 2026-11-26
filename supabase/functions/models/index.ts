// Supabase Edge Function: models
// Endpoint:
//   GET /functions/v1/models -> ModelInfo[] ({ id, label, context_window })
//
// Reads live catalog from `public.app_settings.models` (if present), falling back to
// `OPENMI_MODELS_JSON` secret or the built-in default catalog.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

interface ModelInfo {
  id: string;
  label: string;
  context_window: number;
}

const DEFAULT_MODELS: ModelInfo[] = [
  { id: "openmi-default", label: "Openmi Default", context_window: 128000 },
  { id: "openai/gpt-4o-mini", label: "GPT-4o Mini", context_window: 128000 },
  { id: "openai/gpt-4o", label: "GPT-4o", context_window: 128000 },
  { id: "anthropic/claude-3.5-sonnet", label: "Claude 3.5 Sonnet", context_window: 200000 },
];

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "public, max-age=15",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
    },
  });
}

function isValidModelList(value: unknown): value is ModelInfo[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every(
      (m) =>
        typeof m?.id === "string" &&
        m.id.trim().length > 0 &&
        typeof m?.label === "string" &&
        typeof m?.context_window === "number",
    )
  );
}

function loadEnvModels(): ModelInfo[] {
  const raw = Deno.env.get("OPENMI_MODELS_JSON");
  if (!raw) return DEFAULT_MODELS;
  try {
    const parsed = JSON.parse(raw);
    if (isValidModelList(parsed)) return parsed;
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
      if (data?.models && isValidModelList(data.models)) {
        return jsonResponse(data.models, 200);
      }
    } catch (_) {
      // Fall back to env/default if app_settings is not migrated yet.
    }
  }

  return jsonResponse(loadEnvModels(), 200);
});
