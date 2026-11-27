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
      description:
        "Read a UTF-8 text file inside the user's workspace. Pass optional 1-indexed `offset` and `limit` to read specific line slices of large files.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative path inside the workspace." },
          offset: { type: "integer", description: "Optional 1-indexed starting line number." },
          limit: { type: "integer", description: "Optional maximum number of lines to return." },
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
      name: "grep",
      description:
        "Search file contents across the workspace (.gitignore-aware). Prefer this over run_command for finding symbols or strings.",
      parameters: {
        type: "object",
        properties: {
          pattern: { type: "string", description: "Text pattern to search for." },
          path: { type: "string", description: "Optional relative subdirectory or file (defaults to '.')." },
          include: { type: "string", description: "Optional filename/glob filter such as '*.rs' or '*.ts'." },
          case_sensitive: { type: "boolean", description: "Whether match is case-sensitive (default true)." },
          max_results: { type: "integer", description: "Maximum matching lines to return (default 60, max 200)." },
        },
        required: ["pattern"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "glob",
      description:
        "Find files in the workspace matching a glob pattern (e.g. '**/*.rs', 'src/*.ts'). Respects .gitignore.",
      parameters: {
        type: "object",
        properties: {
          pattern: { type: "string", description: "Glob pattern to match file paths." },
          path: { type: "string", description: "Optional relative subdirectory (defaults to '.')." },
          max_results: { type: "integer", description: "Maximum file paths to return (default 100, max 300)." },
        },
        required: ["pattern"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "edit_file",
      description:
        "Make a targeted exact search-and-replace edit in an existing workspace file. Prefer this over write_file when modifying existing files.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative path inside the workspace." },
          old_string: {
            type: "string",
            description: "Exact existing text to replace (must match uniquely unless replace_all is true).",
          },
          new_string: { type: "string", description: "Replacement text." },
          replace_all: {
            type: "boolean",
            description: "Replace all occurrences instead of requiring a unique match (default false).",
          },
        },
        required: ["path", "old_string", "new_string"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "write_file",
      description: "Create a new file or completely overwrite a file inside the user's workspace.",
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
      name: "delete_file",
      description:
        "Delete an existing file inside the user's workspace (checkpointed so /undo can restore it). Prefer this over `rm` in run_command.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Relative path to the file to delete." },
        },
        required: ["path"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "move_file",
      description:
        "Move or rename a file inside the user's workspace (checkpointed so /undo and /diff track the rename). Prefer this over `mv` in run_command.",
      parameters: {
        type: "object",
        properties: {
          from: { type: "string", description: "Current relative path of the file." },
          to: { type: "string", description: "New relative destination path." },
        },
        required: ["from", "to"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "run_command",
      description:
        "Run a shell command in the user's workspace root. Output is capped at 16 KB (head+tail).",
      parameters: {
        type: "object",
        properties: {
          command: { type: "string", description: "Shell command line to execute." },
          timeout_ms: {
            type: "integer",
            description:
              "Optional timeout in milliseconds (default 120000, max 600000). Kills process group on expiry.",
          },
        },
        required: ["command"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "spawn_agent",
      description:
        "Launch an isolated sub-agent with its own clean message history. Use `explore` (read-only) for verbose codebase searches so only the concise summary enters the main conversation, or `worker` for delegated edits.",
      parameters: {
        type: "object",
        properties: {
          prompt: { type: "string", description: "Self-contained task prompt for the sub-agent." },
          subagent_type: {
            type: "string",
            enum: ["explore", "worker"],
            description: "Sub-agent role: `explore` (read-only tools only) or `worker` (full workspace tools).",
          },
          max_turns: {
            type: "integer",
            description: "Optional maximum steps for the sub-agent (default 12, max 30).",
          },
        },
        required: ["prompt"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "todo_write",
      description:
        "Update the structured task checklist displayed live in the TUI and preserved across context compactions.",
      parameters: {
        type: "object",
        properties: {
          todos: {
            type: "array",
            items: {
              type: "object",
              properties: {
                id: { type: "string", description: "Short unique task identifier." },
                content: { type: "string", description: "Concise task description." },
                status: {
                  type: "string",
                  enum: ["pending", "in_progress", "completed"],
                },
              },
              required: ["id", "content", "status"],
            },
          },
        },
        required: ["todos"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "memory_write",
      description:
        "Persist concise project-specific preferences, build quirks, or architectural notes to external per-repo memory.md (does not dirty git status).",
      parameters: {
        type: "object",
        properties: {
          content: { type: "string", description: "Markdown notes to save." },
          mode: {
            type: "string",
            enum: ["append", "replace"],
            description: "Whether to append to or replace existing memory (default `append`).",
          },
        },
        required: ["content"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "ask_user",
      description:
        "Pause and present an interactive multiple-choice question to the user in the TUI when requirements are ambiguous or a design choice is needed.",
      parameters: {
        type: "object",
        properties: {
          question: { type: "string", description: "Clear, concise question to ask the user." },
          options: {
            type: "array",
            items: { type: "string" },
            description: "List of selectable option labels.",
          },
          allow_custom: {
            type: "boolean",
            description: "Whether to allow a custom write-in answer (default true).",
          },
        },
        required: ["question", "options"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "web_fetch",
      description:
        "Fetch an HTTP(S) web page (read-only), strip HTML tags/scripts into clean Markdown/text, and return up to 16 KB.",
      parameters: {
        type: "object",
        properties: {
          url: { type: "string", description: "HTTP or HTTPS URL to fetch." },
          max_chars: {
            type: "integer",
            description: "Optional character cap (default 16384, max 16384).",
          },
        },
        required: ["url"],
      },
    },
  },
];

const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
  "Access-Control-Expose-Headers":
    "X-Openmi-Plan, X-Openmi-Credits-Remaining, X-Openmi-Wallet-Credit, X-Openmi-Active-Model, X-Openmi-Pass-Expires-At, X-Openmi-Daily-Tokens-Used, X-Openmi-Daily-Token-Limit, X-Openmi-Max-Steps",
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
  let derivedPayUrl = (Deno.env.get("OPENMI_PAY_URL") ?? "").trim();
  if (!derivedPayUrl) {
    const verifyUrl = (Deno.env.get("OPENMI_CLI_VERIFY_URL") ?? "").trim();
    if (/^https?:\/\//i.test(verifyUrl)) {
      try {
        derivedPayUrl = new URL("/pay", verifyUrl).toString();
      } catch (_) {
        // ignore invalid URL
      }
    }
  }
  const upgradeLink = (derivedPayUrl || dbPaymentLink || DEFAULT_PAYMENT_LINK).trim();

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

    if (status === "daily_token_limit_exceeded") {
      const used = Number(res.daily_tokens_used ?? 0);
      const limit = Number(res.daily_token_limit ?? 0);
      const isPaid = Boolean(res.is_paid);
      const upgradeHint = isPaid ? "" : ` or upgrade at ${upgradeLink}`;
      return jsonResponse(
        {
          error: "daily_token_limit_exceeded",
          message: `Daily token limit reached (${used.toLocaleString("en-US")} / ${limit.toLocaleString("en-US")} input+output tokens used today). Resets at 00:00 UTC${upgradeHint}`,
          daily_tokens_used: used,
          daily_token_limit: limit,
          next_reset_at: res.next_reset_at ?? null,
          upgrade_url: upgradeLink,
        },
        429,
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

  // Merge optional client-registered MCP tools (`mcp__*`) with built-in tools.
  const extraTools: unknown[] = Array.isArray(body?.tools)
    ? body.tools
        .filter(
          (t: unknown): t is { type: string; function: { name: string; description?: string; parameters?: unknown } } =>
            Boolean(
              t &&
                typeof t === "object" &&
                (t as { type?: unknown }).type === "function" &&
                typeof (t as { function?: { name?: unknown } }).function?.name === "string" &&
                /^mcp__[A-Za-z0-9_-]+__[A-Za-z0-9_-]+$/.test(
                  (t as { function: { name: string } }).function.name,
                ),
            ),
        )
        .slice(0, 64)
        .map((t) => ({
          type: "function",
          function: {
            name: t.function.name,
            description:
              typeof t.function.description === "string" ? t.function.description.slice(0, 500) : "",
            parameters:
              t.function.parameters && typeof t.function.parameters === "object"
                ? t.function.parameters
                : { type: "object", properties: {} },
          },
        }))
    : [];
  const allowedSet = Array.isArray(body?.allowed_tools)
    ? new Set(
        body.allowed_tools.filter((t: unknown): t is string => typeof t === "string"),
      )
    : null;
  const filteredBuiltins = allowedSet
    ? BUILTIN_TOOLS.filter((t) => allowedSet.has(t.function.name))
    : BUILTIN_TOOLS;
  const allTools = [...filteredBuiltins, ...extraTools];

  // Estimate input characters for fallback token counting when upstream omits `usage`.
  const inputChars =
    messages.reduce((sum, m) => sum + m.role.length + m.content.length + 12, 0) +
    JSON.stringify(allTools).length;

  // 5. Call upstream OpenAI-compatible chat completions API with streaming + usage reporting
  const fetchUpstream = (includeStreamOptions: boolean) =>
    fetch(`${llmBaseUrl}/chat/completions`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${llmApiKey}`,
        Accept: "text/event-stream",
      },
      body: JSON.stringify({
        model: upstreamModel,
        stream: true,
        ...(includeStreamOptions ? { stream_options: { include_usage: true } } : {}),
        messages,
        tools: allTools,
        tool_choice: "auto",
      }),
    });

  let upstreamResp: Response;
  try {
    upstreamResp = await fetchUpstream(true);
    if (upstreamResp.status === 400) {
      const probeText = await upstreamResp.clone().text().catch(() => "");
      if (/stream_options/i.test(probeText)) {
        upstreamResp = await fetchUpstream(false);
      }
    }
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

  // 6. Translate upstream OpenAI SSE stream into Openmi SSE chunks and record input+output tokens
  const upstreamReader = upstreamResp.body.getReader();
  const decoder = new TextDecoder();

  let reportedTokens = 0;
  let outputChars = 0;
  let usageRecorded = false;

  const commitTokenUsage = async () => {
    if (usageRecorded) return;
    usageRecorded = true;
    const tokensToRecord =
      reportedTokens > 0
        ? reportedTokens
        : Math.max(1, Math.ceil((inputChars + outputChars) / 4));
    try {
      await admin.rpc("record_token_usage", {
        p_user_id: userId,
        p_tokens: tokensToRecord,
      });
    } catch (_) {
      // Best-effort recording; never break an already-streaming response.
    }
  };

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

            // Capture OpenAI-compatible usage chunk (prompt_tokens + completion_tokens).
            if (parsed.usage && typeof parsed.usage === "object") {
              const u = parsed.usage as Record<string, unknown>;
              const promptTok =
                typeof u.prompt_tokens === "number" && Number.isFinite(u.prompt_tokens)
                  ? Math.max(0, Math.floor(u.prompt_tokens))
                  : typeof u.input_tokens === "number" && Number.isFinite(u.input_tokens)
                    ? Math.max(0, Math.floor(u.input_tokens))
                    : 0;
              const completionTok =
                typeof u.completion_tokens === "number" && Number.isFinite(u.completion_tokens)
                  ? Math.max(0, Math.floor(u.completion_tokens))
                  : typeof u.output_tokens === "number" && Number.isFinite(u.output_tokens)
                    ? Math.max(0, Math.floor(u.output_tokens))
                    : 0;
              const totalTok =
                typeof u.total_tokens === "number" && Number.isFinite(u.total_tokens)
                  ? Math.max(0, Math.floor(u.total_tokens))
                  : 0;
              const combined = promptTok + completionTok;
              if (combined > 0 || totalTok > 0) {
                reportedTokens = Math.max(combined, totalTok);
              }
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
              await commitTokenUsage();
              controller.close();
              return;
            }

            const choices = parsed.choices as Array<Record<string, unknown>> | undefined;
            const choice = choices?.[0];
            if (!choice) continue;

            const delta = choice.delta as Record<string, unknown> | undefined;
            if (delta) {
              if (typeof delta.content === "string" && delta.content.length > 0) {
                outputChars += delta.content.length;
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
                    if (typeof fn.name === "string" && fn.name) {
                      outputChars += fn.name.length;
                      entry.name += fn.name;
                    }
                    if (typeof fn.arguments === "string" && fn.arguments) {
                      outputChars += fn.arguments.length;
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
        await commitTokenUsage();
        controller.enqueue(sseLine({ type: "done", finish_reason: finishReason }));
      } catch (err) {
        await commitTokenUsage();
        controller.enqueue(
          sseLine({
            type: "error",
            code: "stream_error",
            message: err instanceof Error ? err.message : String(err),
          }),
        );
      } finally {
        await commitTokenUsage();
        controller.close();
      }
    },
    cancel() {
      upstreamReader.cancel().catch(() => {});
      void commitTokenUsage();
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
  if (typeof passMeta.daily_tokens_used === "number") {
    extraHeaders["X-Openmi-Daily-Tokens-Used"] = String(passMeta.daily_tokens_used);
  }
  if (typeof passMeta.daily_token_limit === "number") {
    extraHeaders["X-Openmi-Daily-Token-Limit"] = String(passMeta.daily_token_limit);
  }
  if (typeof passMeta.max_steps === "number") {
    extraHeaders["X-Openmi-Max-Steps"] = String(passMeta.max_steps);
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
