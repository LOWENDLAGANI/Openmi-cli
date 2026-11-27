import { getSupabaseConfig, maskApiKey, readBody, requireAdminAuth, sendJson, supabaseFetch } from "./_supabase.js";

const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";

const DEFAULT_MODELS = [
  { id: "openmi-default", label: "Openmi Default", context_window: 128000, credit_cost: 3, paid_only: false },
  { id: "openai/gpt-4o-mini", label: "GPT-4o Mini", context_window: 128000, credit_cost: 2, paid_only: false },
  { id: "openai/gpt-4o", label: "GPT-4o", context_window: 128000, credit_cost: 3, paid_only: false },
  { id: "anthropic/claude-3.5-sonnet", label: "Claude 3.5 Sonnet", context_window: 200000, credit_cost: 3, paid_only: true },
];

async function fetchRow() {
  const rows = await supabaseFetch("/rest/v1/app_settings?id=eq.1&select=*");
  if (Array.isArray(rows) && rows.length > 0) return rows[0];
  const inserted = await supabaseFetch("/rest/v1/app_settings", {
    method: "POST",
    headers: { Prefer: "return=representation,resolution=merge-duplicates" },
    body: JSON.stringify({ id: 1 }),
  });
  return Array.isArray(inserted) ? inserted[0] : inserted;
}

// Normalizes the catalog. `existing` is the currently stored catalog so a blank per-model
// `api_key` keeps the previously saved key (keys are never sent back to the browser).
function normalizeModels(list, existing = []) {
  if (!Array.isArray(list) || list.length === 0) return DEFAULT_MODELS;
  const storedKeys = new Map(
    (Array.isArray(existing) ? existing : [])
      .filter((m) => m && typeof m.id === "string")
      .map((m) => [m.id.trim(), String(m.api_key || "")]),
  );
  return list
    .filter((m) => m && typeof m.id === "string" && m.id.trim())
    .map((m) => {
      const id = m.id.trim();
      let apiKey = typeof m.api_key === "string" ? m.api_key.trim() : "";
      if (m.clear_api_key === true) apiKey = "";
      else if (!apiKey) apiKey = storedKeys.get(id) || "";
      const baseUrl = typeof m.base_url === "string" ? m.base_url.trim().replace(/\/+$/, "") : "";
      return {
        id,
        label: String(m.label || m.display_name || m.id).trim(),
        display_name: String(m.display_name || "").trim(),
        description: String(m.description || "").trim().slice(0, 140),
        base_url: /^https?:\/\//i.test(baseUrl) ? baseUrl : "",
        api_key: apiKey,
        _rest: m,
      };
    })
    .map(({ _rest: m, ...base }) => ({
      ...base,
      context_window: Math.max(1000, Math.floor(Number(m.context_window) || 128000)),
      credit_cost:
        typeof m.credit_cost === "number" && Number.isFinite(m.credit_cost) && m.credit_cost >= 0
          ? Math.floor(m.credit_cost)
          : 3,
      paid_only: Boolean(m.paid_only),
    }));
}

// Public/admin-safe view of a model: secrets are replaced by `has_api_key` + masked preview.
function publicModel(m) {
  const { api_key, ...rest } = m;
  const key = String(api_key || "");
  return { ...rest, has_api_key: key.length > 0, api_key_masked: maskApiKey(key) };
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
    credit_cost: typeof row?.credit_cost === "number" ? row.credit_cost : 3,
    starter_credits: typeof row?.starter_credits === "number" ? row.starter_credits : 6,
    free_daily_credits: typeof row?.free_daily_credits === "number" ? row.free_daily_credits : 6,
    paid_daily_credits: typeof row?.paid_daily_credits === "number" ? row.paid_daily_credits : 15,
    free_daily_token_limit: typeof row?.free_daily_token_limit === "number" ? row.free_daily_token_limit : 2000000,
    paid_daily_token_limit: typeof row?.paid_daily_token_limit === "number" ? row.paid_daily_token_limit : 7000000,
    free_max_steps: typeof row?.free_max_steps === "number" ? row.free_max_steps : 40,
    paid_max_steps: typeof row?.paid_max_steps === "number" ? row.paid_max_steps : 70,
    pass_duration_minutes: typeof row?.pass_duration_minutes === "number" ? row.pass_duration_minutes : 60,
    payment_link: row?.payment_link || DEFAULT_PAYMENT_LINK,
    topup_paused: Boolean(row?.topup_paused),
    models: normalizeModels(row?.models, row?.models).map(publicModel),
    updated_at: row?.updated_at || null,
  };
}

export default async function handler(req, res) {
  try {
    if (!requireAdminAuth(req, res)) return;
    const { url: supabaseUrl } = getSupabaseConfig();

    if (req.method === "GET") {
      const row = await fetchRow();
      return sendJson(res, 200, formatSettings(row, supabaseUrl));
    }

    if (req.method === "POST" || req.method === "PATCH") {
      const body = await readBody(req);

      if (body.action === "test_llm") {
        const row = await fetchRow();
        const savedBaseUrl = String(row?.llm_base_url || "https://openrouter.ai/api/v1").replace(
          /\/+$/,
          "",
        );
        const requestedBaseUrl = typeof body.llm_base_url === "string" && body.llm_base_url.trim()
          ? body.llm_base_url.trim().replace(/\/+$/, "")
          : savedBaseUrl;
        if (!/^https:\/\//i.test(requestedBaseUrl)) {
          return sendJson(res, 400, { ok: false, error: "llm_base_url must be an https:// URL." });
        }
        const suppliedKey = typeof body.llm_api_key === "string" ? body.llm_api_key.trim() : "";
        // Never transmit the stored API key to a different (unsaved) host to prevent key exfiltration.
        if (!suppliedKey && requestedBaseUrl !== savedBaseUrl) {
          return sendJson(res, 400, {
            ok: false,
            error: "Provide an API key (or save the base URL first) when testing a different host.",
          });
        }
        const key = suppliedKey || String(row?.llm_api_key || "").trim();
        if (!key) {
          return sendJson(res, 400, { ok: false, error: "No API key configured to test." });
        }
        const t0 = Date.now();
        const probe = await fetch(`${requestedBaseUrl}/models`, {
          headers: { Authorization: `Bearer ${key}` },
          signal: AbortSignal.timeout(10000),
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
      if (typeof body.topup_paused === "boolean") patch.topup_paused = body.topup_paused;
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
      if (typeof body.payment_link === "string" && /^https?:\/\//i.test(body.payment_link.trim())) {
        patch.payment_link = body.payment_link.trim();
      }
      if (body.credit_cost !== undefined) {
        const cost = Number(body.credit_cost);
        if (!Number.isFinite(cost) || cost < 0) {
          return sendJson(res, 400, { error: "credit_cost must be >= 0" });
        }
        patch.credit_cost = Math.floor(cost);
      }
      if (body.free_daily_credits !== undefined) {
        const v = Number(body.free_daily_credits);
        if (!Number.isFinite(v) || v < 0) {
          return sendJson(res, 400, { error: "free_daily_credits must be >= 0" });
        }
        patch.free_daily_credits = Math.floor(v);
        patch.starter_credits = Math.floor(v);
      }
      if (body.paid_daily_credits !== undefined) {
        const v = Number(body.paid_daily_credits);
        if (!Number.isFinite(v) || v < 0) {
          return sendJson(res, 400, { error: "paid_daily_credits must be >= 0" });
        }
        patch.paid_daily_credits = Math.floor(v);
      }
      if (body.free_daily_token_limit !== undefined) {
        const v = Number(body.free_daily_token_limit);
        if (!Number.isFinite(v) || v < 0) {
          return sendJson(res, 400, { error: "free_daily_token_limit must be >= 0" });
        }
        patch.free_daily_token_limit = Math.floor(v);
      }
      if (body.paid_daily_token_limit !== undefined) {
        const v = Number(body.paid_daily_token_limit);
        if (!Number.isFinite(v) || v < 0) {
          return sendJson(res, 400, { error: "paid_daily_token_limit must be >= 0" });
        }
        patch.paid_daily_token_limit = Math.floor(v);
      }
      if (body.free_max_steps !== undefined) {
        const v = Number(body.free_max_steps);
        if (!Number.isFinite(v) || v < 1) {
          return sendJson(res, 400, { error: "free_max_steps must be >= 1" });
        }
        patch.free_max_steps = Math.floor(v);
      }
      if (body.paid_max_steps !== undefined) {
        const v = Number(body.paid_max_steps);
        if (!Number.isFinite(v) || v < 1) {
          return sendJson(res, 400, { error: "paid_max_steps must be >= 1" });
        }
        patch.paid_max_steps = Math.floor(v);
      }
      if (body.pass_duration_minutes !== undefined) {
        const v = Number(body.pass_duration_minutes);
        if (!Number.isFinite(v) || v < 1) {
          return sendJson(res, 400, { error: "pass_duration_minutes must be >= 1" });
        }
        patch.pass_duration_minutes = Math.floor(v);
      }
      if (Array.isArray(body.models)) {
        const current = await fetchRow();
        const cleaned = normalizeModels(body.models, current?.models);
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
