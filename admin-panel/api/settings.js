import { getSupabaseConfig, maskApiKey, readBody, sendJson, supabaseFetch } from "./_supabase.js";

const DEFAULT_MODELS = [
  { id: "openmi-default", label: "Openmi Default", context_window: 128000 },
  { id: "openai/gpt-4o-mini", label: "GPT-4o Mini", context_window: 128000 },
  { id: "openai/gpt-4o", label: "GPT-4o", context_window: 128000 },
  { id: "anthropic/claude-3.5-sonnet", label: "Claude 3.5 Sonnet", context_window: 200000 },
];

async function fetchRow() {
  const rows = await supabaseFetch("/rest/v1/app_settings?id=eq.1&select=*");
  if (Array.isArray(rows) && rows.length > 0) return rows[0];
  // Auto-seed row if table exists but is empty.
  const inserted = await supabaseFetch("/rest/v1/app_settings", {
    method: "POST",
    headers: { Prefer: "return=representation,resolution=merge-duplicates" },
    body: JSON.stringify({ id: 1 }),
  });
  return Array.isArray(inserted) ? inserted[0] : inserted;
}

function formatSettings(row, supabaseUrl) {
  const apiKey = String(row?.llm_api_key || "");
  return {
    supabase_url: supabaseUrl,
    ai_enabled: row?.ai_enabled !== false,
    llm_base_url: row?.llm_base_url || "https://openrouter.ai/api/v1",
    has_api_key: apiKey.trim().length > 0,
    llm_api_key_masked: maskApiKey(apiKey),
    llm_default_model: row?.llm_default_model || "openai/gpt-4o-mini",
    credit_cost: typeof row?.credit_cost === "number" ? row.credit_cost : 1,
    starter_credits: typeof row?.starter_credits === "number" ? row.starter_credits : 100,
    models: Array.isArray(row?.models) && row.models.length > 0 ? row.models : DEFAULT_MODELS,
    updated_at: row?.updated_at || null,
  };
}

export default async function handler(req, res) {
  try {
    const { url: supabaseUrl } = getSupabaseConfig();

    if (req.method === "GET") {
      const row = await fetchRow();
      return sendJson(res, 200, formatSettings(row, supabaseUrl));
    }

    if (req.method === "POST" || req.method === "PATCH") {
      const body = await readBody(req);

      if (body.action === "test_llm") {
        const row = await fetchRow();
        const baseUrl = String(body.llm_base_url || row?.llm_base_url || "https://openrouter.ai/api/v1").replace(
          /\/+$/,
          "",
        );
        const key = String(body.llm_api_key || row?.llm_api_key || "").trim();
        if (!key) {
          return sendJson(res, 400, { ok: false, error: "No API key configured to test." });
        }
        const t0 = Date.now();
        const probe = await fetch(`${baseUrl}/models`, {
          headers: { Authorization: `Bearer ${key}` },
        });
        const latencyMs = Date.now() - t0;
        if (!probe.ok) {
          const errText = (await probe.text().catch(() => "")).slice(0, 240);
          return sendJson(res, 200, {
            ok: false,
            status: probe.status,
            latency_ms: latencyMs,
            error: errText || probe.statusText,
          });
        }
        return sendJson(res, 200, { ok: true, status: probe.status, latency_ms: latencyMs });
      }

      const patch = { updated_at: new Date().toISOString() };
      if (typeof body.ai_enabled === "boolean") patch.ai_enabled = body.ai_enabled;
      if (typeof body.llm_base_url === "string" && body.llm_base_url.trim()) {
        patch.llm_base_url = body.llm_base_url.trim().replace(/\/+$/, "");
      }
      if (body.clear_api_key === true) {
        patch.llm_api_key = "";
      } else if (typeof body.llm_api_key === "string" && body.llm_api_key.trim()) {
        patch.llm_api_key = body.llm_api_key.trim();
      }
      if (typeof body.llm_default_model === "string" && body.llm_default_model.trim()) {
        patch.llm_default_model = body.llm_default_model.trim();
      }
      if (body.credit_cost !== undefined) {
        const cost = Number(body.credit_cost);
        if (!Number.isFinite(cost) || cost < 0) {
          return sendJson(res, 400, { error: "credit_cost must be >= 0" });
        }
        patch.credit_cost = Math.floor(cost);
      }
      if (body.starter_credits !== undefined) {
        const starter = Number(body.starter_credits);
        if (!Number.isFinite(starter) || starter < 0) {
          return sendJson(res, 400, { error: "starter_credits must be >= 0" });
        }
        patch.starter_credits = Math.floor(starter);
      }
      if (Array.isArray(body.models)) {
        const cleaned = body.models
          .filter((m) => m && typeof m.id === "string" && m.id.trim())
          .map((m) => ({
            id: m.id.trim(),
            label: String(m.label || m.id).trim(),
            context_window: Math.max(1000, Math.floor(Number(m.context_window) || 128000)),
          }));
        if (cleaned.length === 0) {
          return sendJson(res, 400, { error: "At least one model is required." });
        }
        patch.models = cleaned;
      }

      const updated = await supabaseFetch("/rest/v1/app_settings?id=eq.1", {
        method: "PATCH",
        headers: { Prefer: "return=representation" },
        body: JSON.stringify(patch),
      });
      const row = Array.isArray(updated) && updated.length > 0 ? updated[0] : await fetchRow();
      return sendJson(res, 200, formatSettings(row, supabaseUrl));
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message || "Internal server error" });
  }
}
