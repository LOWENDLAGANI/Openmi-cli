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
// Required secret:
//   LLM_API_KEY               -> API key for your OpenAI-compatible LLM provider
// Optional secrets:
//   LLM_BASE_URL              -> Defaults to "https://openrouter.ai/api/v1"
//   LLM_DEFAULT_MODEL         -> Model used when CLI sends "openmi-default" (default: "openai/gpt-4o-mini")
//   OPENMI_CREDIT_COST        -> Credits deducted per step (default: 1; set 0 to disable)
//   OPENMI_STARTER_CREDITS    -> Starter credits auto-provisioned for new users (default: 100)

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const DEFAULT_BASE_URL = "https://openrouter.ai/api/v1";
const DEFAULT_MODEL = "openai/gpt-4o-mini";

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

function normalizeMessages(raw: unknown): Array<{ role: string; content: string }> {
  if (!Array.isArray(raw)) return [];
  const out: Array<{ role: string; content: string }> = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const msg = item as Partial<IncomingMessage>;
    const role = typeof msg.role === "string" ? msg.role.toLowerCase() : "user";
    const content = typeof msg.content === "string" ? msg.content : "";
    if (role === "tool") {
      // Openmi's wire schema sends `{ role: "tool", content }` without `tool_call_id`.
      // Translating to a labeled user message keeps all OpenAI-compatible providers happy.
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
  let dbCreditCost: number | null = null;
  let dbStarterCredits: number | null = null;

  try {
    const { data: cfg } = await admin
      .from("app_settings")
      .select("ai_enabled, llm_base_url, llm_api_key, llm_default_model, credit_cost, starter_credits")
      .eq("id", 1)
      .maybeSingle();
    if (cfg) {
      if (typeof cfg.ai_enabled === "boolean") aiEnabled = cfg.ai_enabled;
      if (typeof cfg.llm_api_key === "string") dbApiKey = cfg.llm_api_key.trim();
      if (typeof cfg.llm_base_url === "string") dbBaseUrl = cfg.llm_base_url.trim();
      if (typeof cfg.llm_default_model === "string") dbDefaultModel = cfg.llm_default_model.trim();
      if (typeof cfg.credit_cost === "number") dbCreditCost = cfg.credit_cost;
      if (typeof cfg.starter_credits === "number") dbStarterCredits = cfg.starter_credits;
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

  const llmApiKey = dbApiKey || (Deno.env.get("LLM_API_KEY") ?? "").trim();
  if (!llmApiKey) {
    return jsonResponse(
      {
        error: "llm_not_configured",
        message: "Server missing LLM API key. Set it in the Admin Panel or Supabase Secrets.",
      },
      503,
    );
  }
  const llmBaseUrl = (dbBaseUrl || Deno.env.get("LLM_BASE_URL") || DEFAULT_BASE_URL).replace(
    /\/+$/,
    "",
  );
  const defaultModel =
    (dbDefaultModel || Deno.env.get("LLM_DEFAULT_MODEL") || DEFAULT_MODEL).trim() || DEFAULT_MODEL;

  // 3. Parse request body
  const body = await req.json().catch(() => null);
  if (!body || typeof body !== "object") {
    return jsonResponse({ error: "invalid_request", message: "Expected JSON body" }, 400);
  }
  const messages = normalizeMessages((body as Record<string, unknown>).messages);
  if (messages.length === 0) {
    return jsonResponse({ error: "invalid_request", message: "messages array is required" }, 400);
  }

  const requestedModel = String((body as Record<string, unknown>).model ?? "").trim();
  const upstreamModel =
    !requestedModel || requestedModel === "openmi-default" ? defaultModel : requestedModel;

  // 4. Ensure user_credits row exists and atomically deduct step cost
  const creditCostRaw =
    dbCreditCost !== null ? dbCreditCost : Number(Deno.env.get("OPENMI_CREDIT_COST") ?? "1");
  const creditCost = Number.isFinite(creditCostRaw) && creditCostRaw >= 0 ? Math.floor(creditCostRaw) : 1;
  const starterCreditsRaw =
    dbStarterCredits !== null
      ? dbStarterCredits
      : Number(Deno.env.get("OPENMI_STARTER_CREDITS") ?? "100");
  const starterCredits = Number.isFinite(starterCreditsRaw) ? Math.floor(starterCreditsRaw) : 100;

  if (creditCost > 0) {
    await admin
      .from("user_credits")
      .upsert(
        { user_id: userId, plan: "free", balance: starterCredits },
        { onConflict: "user_id", ignoreDuplicates: true },
      );

    const { data: newBalance, error: deductErr } = await admin.rpc("deduct_user_credits", {
      p_user_id: userId,
      p_amount: creditCost,
    });

    if (deductErr) {
      return jsonResponse({ error: "credit_check_failed", message: deductErr.message }, 500);
    }
    if (newBalance === null || newBalance === undefined) {
      return jsonResponse({ error: "out_of_credits", message: "Insufficient credits" }, 402);
    }
  }

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
    return jsonResponse(
      { error: "upstream_unreachable", message: err instanceof Error ? err.message : String(err) },
      502,
    );
  }

  if (!upstreamResp.ok || !upstreamResp.body) {
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

  return new Response(stream, {
    status: 200,
    headers: {
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-cache, no-store",
      Connection: "keep-alive",
      ...CORS_HEADERS,
    },
  });
});
