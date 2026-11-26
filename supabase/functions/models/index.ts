// Supabase Edge Function: models
// Endpoint:
//   GET /functions/v1/models -> ModelInfo[] ({ id, label, context_window })
//
// Optional secret:
//   OPENMI_MODELS_JSON -> JSON array of { id, label, context_window } to override the default catalog.

interface ModelInfo {
  id: string;
  label: string;
  context_window: number;
}

const DEFAULT_MODELS: ModelInfo[] = [
  {
    id: "openmi-default",
    label: "Openmi Default",
    context_window: 128000,
  },
  {
    id: "openai/gpt-4o-mini",
    label: "GPT-4o Mini",
    context_window: 128000,
  },
  {
    id: "openai/gpt-4o",
    label: "GPT-4o",
    context_window: 128000,
  },
  {
    id: "anthropic/claude-3.5-sonnet",
    label: "Claude 3.5 Sonnet",
    context_window: 200000,
  },
];

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "public, max-age=60",
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
      "Access-Control-Allow-Methods": "GET, OPTIONS",
    },
  });
}

function loadModels(): ModelInfo[] {
  const raw = Deno.env.get("OPENMI_MODELS_JSON");
  if (!raw) return DEFAULT_MODELS;
  try {
    const parsed = JSON.parse(raw);
    if (
      Array.isArray(parsed) &&
      parsed.length > 0 &&
      parsed.every(
        (m) =>
          typeof m?.id === "string" &&
          typeof m?.label === "string" &&
          typeof m?.context_window === "number",
      )
    ) {
      return parsed;
    }
  } catch (_) {
    // Fall back to defaults if OPENMI_MODELS_JSON is malformed.
  }
  return DEFAULT_MODELS;
}

Deno.serve((req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }
  if (req.method !== "GET") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }
  return jsonResponse(loadModels(), 200);
});
