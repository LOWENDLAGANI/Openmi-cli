// Supabase Edge Function: chat
// Endpoint:
//   POST /functions/v1/chat
//   Headers: Authorization: Bearer <supabase_access_token>
//   Body:    { model: string, stream?: boolean, messages: Array<{ role, content }> }
//   Returns: text/event-stream emitting Openmi SSE chunks:
//     data: {"type":"text_delta","text":"..."}
//     data: {"type":"tool_call","call_id":"...","tool_name":"...","arguments":{...}}
//     data: {"type":"done","finish_reason":"stop"|"tool_calls"|...}
//     data: {"type":"error","code":"...","message":"..."}
//
// Billing model:
//   - Free tier: 6 credits/day (00:00 UTC reset, no rollover)
//   - Paid pass (paid_until > now): 15 credits/day + unlocks paid_only models
//   - Wallet credit: never expires; spent only after daily balance is exhausted
//   - Hourly model pass: first prompt to a model deducts its hourly credit_cost and
//     grants 60 minutes (server time) of unlimited prompts to that model.
//     Single active pass at a time (prompting another model replaces the previous pass).
//   - Automatic metering refund: if upstream LLM fails on the exact request that started
//     a brand-new pass, refund_hour_session returns exact daily & wallet portions and ends the hour.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const DEFAULT_BASE_URL = "https://openrouter.ai/api/v1";
const DEFAULT_MODEL = "openai/gpt-4o-mini";
const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";

const BUILTIN_TOOLS = [
  {
    type: "function",
    function: {
      name: "read_file",
      description: "Read a UTF-8 text file inside the user's workspace.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative path inside the workspace." },
        },
        required: ["path"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "list_dir",
      description: "List files and subdirectories in a workspace directory.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative directory path (defaults to '.')." },
        },
      },
    },
  },
  {
    type: "function",
    function: {
      name: "write_file",
      description: "Create or overwrite a file inside the user's workspace.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative path inside the workspace." },
          content: { type: "string", description: "Full file contents to write." },
        },
        required: ["path", "content"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "run_command",
      description: "Run a shell command in the user's workspace root.",
      parameters: {
        type: "object",
        properties: {
          command: { type: "string", description: "Shell command line to execute." },
        },
        required: ["command"],
      },
    },
  },
];

const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
  "Access-Control-Expose-Headers":
    "X-Openmi-Plan, X-Openmi-Credits-Remaining, X-Openmi-Wallet-Credit, X-Openmi-Active-Model, X-Openmi-Pass-Expires-At",
};

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      ...CORS_HEADERS,
    },
  });
}

function sseLine(payload: unknown): Uint8Array {
  return new TextEncoder().encode(`data: ${JSON.stringify(payload)}\n\n`);
}

interface IncomingMessage {
  role: string;
  content: string;
}

interface CatalogModel {
  id: string;
  label?: string;
  credit_cost?: number;
  paid_only?: boolean;
  base_url?: string;
  api_key?: string;
}

function normalizeMessages(raw: unknown): Array<{ role: string; content: string }> {
  if (!Array.isArray(raw)) return [];
  const out: Array<{ role: string; content: string }> = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const msg = item as Partial<IncomingMessage>;
    const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "user";
    const content = typeof msg.content === "string" ? msg.content : "";
    if (role === "tool") {
      out.push({ role: "user", content: `[Tool Result]\n${content}` });
    } else if (role === "system" || role === "user" || role === "assistant") {
      out.push({ role, content });
    } else {
      out.push({ role: "user", content });
    }
  }
  return out;
}

interface PendingToolCall {
  id: string;
  name: string;
  arguments: string;
}

function resolveModelPricing(
  catalog: CatalogModel[],
  requestedModel: string,
  upstreamModel: string,
  fallbackCost: number,
): { passModelId: string; creditCost: number; paidOnly: boolean } {
  const byRequested = catalog.find((m) => m.id === requestedModel);
  const byUpstream = catalog.find((m) => m.id === upstreamModel);
  const matched = byRequested ?? byUpstream;
  const passModelId = requestedModel || "openmi-default";
  const creditCost =
    matched && typeof matched.credit_cost === "number" && Number.isFinite(matched.credit_cost) && matched.credit_cost >= 0
      ? Math.floor(matched.credit_cost)
      : fallbackCost;
  const paidOnly = Boolean(matched?.paid_only);
  return { passModelId, creditCost, paidOnly };
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }
  if (req.method !== "POST") {
    return jsonResponse({ error: "method_not_allowed" }, 405);
  }

  // 1. Authenticate user
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

  // 2. Load live admin settings from public.app_settings (falling back to env secrets)
  let aiEnabled = true;
  let dbApiKey = "";
  let dbBaseUrl = "";
  let dbDefaultModel = "";
  let dbCreditCost = 3;
  let dbPassDurationMinutes = 60;
  let dbPaymentLink = DEFAULT_PAYMENT_LINK;
  let dbModels: CatalogModel[] = [];

  try {
    const { data: cfg } = await admin
      .from("app_settings")
      .select(
        "ai_enabled, llm_base_url, llm_api_key, llm_default_model, credit_cost, pass_duration_minutes, payment_link, models",
      )
      .eq("id", 1)
      .maybeSingle();
    if (cfg) {
      if (typeof cfg.ai_enabled === "boolean") aiEnabled = cfg.ai_enabled;
      if (typeof cfg.llm_api_key === "string") dbApiKey = cfg.llm_api_key.trim();
      if (typeof cfg.llm_base_url === "string") dbBaseUrl = cfg.llm_base_url.trim();
      if (typeof cfg.llm_default_model === "string") dbDefaultModel = cfg.llm_default_model.trim();
      if (typeof cfg.credit_cost === "number" && cfg.credit_cost >= 0) dbCreditCost = Math.floor(cfg.credit_cost);
      if (typeof cfg.pass_duration_minutes === "number" && cfg.pass_duration_minutes > 0) {
        dbPassDurationMinutes = Math.floor(cfg.pass_duration_minutes);
      }
      if (typeof cfg.payment_link === "string" && /^https?:\/\//i.test(cfg.payment_link.trim())) {
        dbPaymentLink = cfg.payment_link.trim();
      }
      if (Array.isArray(cfg.models)) {
        dbModels = cfg.models as CatalogModel[];
      }
    }
  } catch (_) {
    // Fall back to env secrets if app_settings is not migrated yet.
  }

  if (!aiEnabled) {
    return jsonResponse(
      {
        error: "ai_disabled",
        message: "AI is temporarily paused by the administrator.",
      },
      503,
    );
  }

  const globalApiKey = dbApiKey || (Deno.env.get("LLM_API_KEY") ?? "").trim();
  if (!globalApiKey && !dbModels.some((m) => typeof m.api_key === "string" && m.api_key.trim())) {
    return jsonResponse(
      {
        error: "llm_not_configured",
        message: "Server missing LLM API key. Set it in the Admin Panel or Supabase Secrets.",
      },
      503,
    );
  }
  const globalBaseUrl = (dbBaseUrl || Deno.env.get("LLM_BASE_URL") || DEFAULT_BASE_URL).replace(
    /\/+$/,
    "",
  );
  const defaultModel =
    (dbDefaultModel || Deno.env.get("LLM_DEFAULT_MODEL") || DEFAULT_MODEL).trim() || DEFAULT_MODEL;
  const upgradeLink = (Deno.env.get("OPENMI_PAY_URL") || dbPaymentLink || DEFAULT_PAYMENT_LINK).trim();

  // 3. Parse request body
  const body = await req.json().catch(() => null);
  if (!body || typeof body !== "object") {
    return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
  }
  const messages = normalizeMessages((body as Record<string, unknown>).messages);
  if (messages.length === 0) {
    return jsonResponse({ error: "invalid_request", message: "messages array is required" }, 400);
  }

  const requestedModel = String((body as Record<string, unknown>).model ?? "").trim() || "openmi-default";
  const upstreamModel = requestedModel === "openmi-default" ? defaultModel : requestedModel;
  // Per-model provider override: each catalog model may carry its own base_url + api_key,
  // falling back to the global provider settings when blank.
  const catalogEntry = dbModels.find((m) => m.id === requestedModel) ??
    dbModels.find((m) => m.id === upstreamModel);
  const modelKey = typeof catalogEntry?.api_key === "string" ? catalogEntry.api_key.trim() : "";
  const modelBase = typeof catalogEntry?.base_url === "string"
    ? catalogEntry.base_url.trim().replace(/\/+$/, "")
    : "";
  const llmApiKey = modelKey || globalApiKey;
  const llmBaseUrl = /^https?:\/\//i.test(modelBase) ? modelBase : globalBaseUrl;
  if (!llmApiKey) {
    return jsonResponse(
      {
        error: "llm_not_configured",
        message: `No API key configured for model '${requestedModel}'. Set it in the Admin Panel.`,
      },
      503,
    );
  }
  const { passModelId, creditCost, paidOnly } = resolveModelPricing(
    dbModels,
    requestedModel,
    upstreamModel,
    dbCreditCost,
  );

  // 4. Acquire or reuse 1-hour model pass atomically in Postgres
  let purchasedSessionId: string | null = null;
  let passMeta: Record<string, unknown> = {};

  const { data: passResult, error: passErr } = await admin.rpc("acquire_model_pass", {
    p_user_id: userId,
    p_model_id: passModelId,
    p_credit_cost: creditCost,
    p_paid_only: paidOnly,
    p_duration_minutes: dbPassDurationMinutes,
  });

  if (passErr) {
    return jsonResponse({ error: "credit_check_failed", message: passErr.message }, 500);
  }

  if (passResult && typeof passResult === "object") {
    const res = passResult as Record<string, unknown>;
    passMeta = res;
    const status = String(res.status ?? "");

    if (status === "plan_required") {
      return jsonResponse(
        {
          error: "plan_required",
          message: `Model '${passModelId}' requires an active Paid pass. Upgrade at ${upgradeLink}`,
          upgrade_url: upgradeLink,
        },
        403,
      );
    }

    if (status === "insufficient_credits") {
      const bal = Number(res.balance ?? 0);
      const wallet = Number(res.wallet_credit ?? 0);
      return jsonResponse(
        {
          error: "out_of_credits",
          message: `Insufficient credits for '${passModelId}' (needs ${creditCost} cr/hr; you have ${bal} daily + ${wallet} wallet). Resets at 00:00 UTC or upgrade at ${upgradeLink}`,
          balance: bal,
          wallet_credit: wallet,
          required: creditCost,
          next_reset_at: res.next_reset_at ?? null,
          upgrade_url: upgradeLink,
        },
        402,
      );
    }

    if (status === "purchased" && typeof res.session_id === "string") {
      purchasedSessionId = res.session_id;
    }
  }

  const refundIfNewlyPurchased = async () => {
    if (!purchasedSessionId) return;
    try {
      await admin.rpc("refund_hour_session", {
        p_user_id: userId,
        p_session_id: purchasedSessionId,
      });
    } catch (_) {
      // Best-effort refund; never mask original upstream error.
    }
  };

  // 5. Call upstream OpenAI-compatible chat completions API with streaming
  let upstreamResp: Response;
  try {
    upstreamResp = await fetch(`${llmBaseUrl}/chat/completions`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${llmApiKey}`,
        Accept: "text/event-stream",
      },
      body: JSON.stringify({
        model: upstreamModel,
        stream: true,
        messages,
        tools: BUILTIN_TOOLS,
        tool_choice: "auto",
      }),
    });
  } catch (err) {
    await refundIfNewlyPurchased();
    return jsonResponse(
      { error: "upstream_unreachable", message: err instanceof Error ? err.message : String(err) },
      502,
    );
  }

  if (!upstreamResp.ok || !upstreamResp.body) {
    await refundIfNewlyPurchased();
    const errText = await upstreamResp.text().catch(() => "");
    return jsonResponse(
      {
        error: "upstream_error",
        status: upstreamResp.status,
        message: errText.slice(0, 500) || upstreamResp.statusText,
      },
      upstreamResp.status >= 400 && upstreamResp.status < 500 ? upstreamResp.status : 502,
    );
  }

  // 6. Translate upstream OpenAI SSE stream into Openmi SSE chunks
  const upstreamReader = upstreamResp.body.getReader();
  const decoder = new TextDecoder();

  const stream = new ReadableStream<Uint8Array>({
    async start(controller) {
      let buffer = "";
      let finishReason = "stop";
      const pendingCalls = new Map<number, PendingToolCall>();

      const flushToolCalls = () => {
        const sorted = [...pendingCalls.entries()].sort((a, b) => a[0] - b[0]);
        for (const [idx, tc] of sorted) {
          if (!tc.name) continue;
          let parsedArgs: unknown = {};
          try {
            parsedArgs = tc.arguments.trim() ? JSON.parse(tc.arguments) : {};
          } catch (_) {
            parsedArgs = { raw: tc.arguments };
          }
          controller.enqueue(
            sseLine({
              type: "tool_call",
              call_id: tc.id || `call_${idx}`,
              tool_name: tc.name,
              arguments: parsedArgs,
            }),
          );
        }
        pendingCalls.clear();
      };

      try {
        while (true) {
          const { done, value } = await upstreamReader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });

          let nl: number;
          while ((nl = buffer.indexOf("\n")) !== -1) {
            const line = buffer.slice(0, nl).replace(/\r$/, "");
            buffer = buffer.slice(nl + 1);
            if (!line.startsWith("data:")) continue;
            const data = line.slice(5).trim();
            if (!data || data === "[DONE]") continue;

            let parsed: Record<string, unknown>;
            try {
              parsed = JSON.parse(data);
            } catch (_) {
              continue;
            }

            if (parsed.error && typeof parsed.error === "object") {
              const e = parsed.error as Record<string, unknown>;
              controller.enqueue(
                sseLine({
                  type: "error",
                  code: String(e.code ?? "upstream_error"),
                  message: String(e.message ?? "Upstream stream error"),
                }),
              );
              controller.close();
              return;
            }

            const choices = parsed.choices as Array<Record<string, unknown>> | undefined;
            const choice = choices?.[0];
            if (!choice) continue;

            const delta = choice.delta as Record<string, unknown> | undefined;
            if (delta) {
              if (typeof delta.content === "string" && delta.content.length > 0) {
                controller.enqueue(sseLine({ type: "text_delta", text: delta.content }));
              }
              const tcDeltas = delta.tool_calls as Array<Record<string, unknown>> | undefined;
              if (Array.isArray(tcDeltas)) {
                for (const tcd of tcDeltas) {
                  const idx = typeof tcd.index === "number" ? tcd.index : 0;
                  const entry = pendingCalls.get(idx) ?? { id: "", name: "", arguments: "" };
                  if (typeof tcd.id === "string" && tcd.id) entry.id = tcd.id;
                  const fn = tcd.function as Record<string, unknown> | undefined;
                  if (fn) {
                    if (typeof fn.name === "string" && fn.name) entry.name += fn.name;
                    if (typeof fn.arguments === "string" && fn.arguments) {
                      entry.arguments += fn.arguments;
                    }
                  }
                  pendingCalls.set(idx, entry);
                }
              }
            }

            if (typeof choice.finish_reason === "string" && choice.finish_reason) {
              finishReason = choice.finish_reason;
            }
          }
        }

        flushToolCalls();
        controller.enqueue(sseLine({ type: "done", finish_reason: finishReason }));
      } catch (err) {
        controller.enqueue(
          sseLine({
            type: "error",
            code: "stream_error",
            message: err instanceof Error ? err.message : String(err),
          }),
        );
      } finally {
        controller.close();
      }
    },
    cancel() {
      upstreamReader.cancel().catch(() => {});
    },
  });

  const extraHeaders: Record<string, string> = {};
  if (typeof passMeta.plan === "string") extraHeaders["X-Openmi-Plan"] = passMeta.plan;
  if (typeof passMeta.balance === "number") {
    extraHeaders["X-Openmi-Credits-Remaining"] = String(passMeta.balance);
  }
  if (typeof passMeta.wallet_credit === "number") {
    extraHeaders["X-Openmi-Wallet-Credit"] = String(passMeta.wallet_credit);
  }
  if (typeof passMeta.active_model === "string") {
    extraHeaders["X-Openmi-Active-Model"] = passMeta.active_model;
  }
  if (typeof passMeta.pass_expires_at === "string") {
    extraHeaders["X-Openmi-Pass-Expires-At"] = passMeta.pass_expires_at;
  }

  return new Response(stream, {
    status: 200,
    headers: {
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-cache, no-store",
      Connection: "keep-alive",
      ...CORS_HEADERS,
      ...extraHeaders,
    },
  });
});
