// Supabase Edge Function: payments
// Endpoints:
//   GET  /functions/v1/payments -> Plans catalogue, credit/pass summary, and user's last 20
//                                  payment_requests with 300-second signed receipt URLs.
//   POST /functions/v1/payments -> Multipart proof-of-payment receipt upload:
//                                  - Rate-limited (5 uploads/minute per account)
//                                  - Validates plan (pass-15 / pass-30), reference, amount, note
//                                  - Allowlists MIME (png, jpeg, webp, pdf) <= 5 MB + magic-byte check
//                                  - Uploads to private `receipts` bucket as `${userId}/${uuid}.${ext}`
//                                  - Calls `submit_payment_request` RPC (deletes orphan file on failure)
//                                  - Optionally notifies operator via Resend (non-fatal on mail failure)

import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";
const MAX_RECEIPT_BYTES = 5 * 1024 * 1024; // 5 MB

interface PlanDef {
  id: "pass-15" | "pass-30";
  name: string;
  price_myr: number;
  days: number;
  upfront_credit: number;
  daily_credits: number;
}

const PLANS: PlanDef[] = [
  {
    id: "pass-15",
    name: "15-day pass",
    price_myr: 14.99,
    days: 15,
    upfront_credit: 2,
    daily_credits: 15,
  },
  {
    id: "pass-30",
    name: "30-day pass",
    price_myr: 29,
    days: 30,
    upfront_credit: 3,
    daily_credits: 15,
  },
];

const MIME_TO_EXT: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/webp": "webp",
  "application/pdf": "pdf",
};

const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, apikey, content-type, x-client-info",
  "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
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

function safeHttpUrl(raw: unknown, fallback: string): string {
  const s = typeof raw === "string" ? raw.trim() : "";
  if (/^https?:\/\//i.test(s)) return s;
  return fallback;
}

function receiptBytesMatchType(bytes: Uint8Array, mime: string): boolean {
  if (bytes.length < 12) return false;
  if (mime === "image/png") {
    // 89 50 4E 47 0D 0A 1A 0A
    return (
      bytes[0] === 0x89 &&
      bytes[1] === 0x50 &&
      bytes[2] === 0x4e &&
      bytes[3] === 0x47 &&
      bytes[4] === 0x0d &&
      bytes[5] === 0x0a &&
      bytes[6] === 0x1a &&
      bytes[7] === 0x0a
    );
  }
  if (mime === "image/jpeg") {
    // FF D8 FF
    return bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff;
  }
  if (mime === "image/webp") {
    // "RIFF" .... "WEBP"
    return (
      bytes[0] === 0x52 &&
      bytes[1] === 0x49 &&
      bytes[2] === 0x46 &&
      bytes[3] === 0x46 &&
      bytes[8] === 0x57 &&
      bytes[9] === 0x45 &&
      bytes[10] === 0x42 &&
      bytes[11] === 0x50
    );
  }
  if (mime === "application/pdf") {
    // "%PDF-"
    return (
      bytes[0] === 0x25 &&
      bytes[1] === 0x50 &&
      bytes[2] === 0x44 &&
      bytes[3] === 0x46 &&
      bytes[4] === 0x2d
    );
  }
  return false;
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

async function notifyOperatorEmail(params: {
  payerEmail: string;
  planName: string;
  amountMyr: number;
  reference: string;
  note: string;
  filename: string;
  bytes: Uint8Array;
}): Promise<void> {
  const apiKey = (Deno.env.get("RESEND_API_KEY") ?? "").trim();
  const toEmail = (Deno.env.get("RECEIPT_NOTIFY_EMAIL") ?? "myrealmetvreal@gmail.com").trim();
  const fromEmail = (Deno.env.get("RECEIPT_FROM_EMAIL") ?? "Openmi Billing <billing@openmi.space>").trim();
  if (!apiKey || !toEmail) return;

  try {
    await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        from: fromEmail,
        to: [toEmail],
        reply_to: params.payerEmail || undefined,
        subject: `[Openmi Receipt] ${params.planName} — RM ${params.amountMyr.toFixed(2)} (${params.reference})`,
        text: [
          `New payment receipt awaiting review in Openmi Admin Panel.`,
          ``,
          `Payer:     ${params.payerEmail || "(no email)"}`,
          `Plan:      ${params.planName}`,
          `Amount:    RM ${params.amountMyr.toFixed(2)}`,
          `Reference: ${params.reference}`,
          params.note ? `Note:      ${params.note}` : "",
        ]
          .filter(Boolean)
          .join("\n"),
        attachments: [
          {
            filename: params.filename,
            content: toBase64(params.bytes),
          },
        ],
      }),
    });
  } catch (_) {
    // Mail outages must never fail a receipt submission.
  }
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
  if (!supabaseUrl || !serviceRoleKey) {
    return jsonResponse({ error: "unconfigured", message: "Server billing configuration missing." }, 503);
  }

  const admin = createClient(supabaseUrl, serviceRoleKey, {
    auth: { persistSession: false, autoRefreshToken: false },
  });

  const { data: userData, error: userErr } = await admin.auth.getUser(bearer);
  if (userErr || !userData?.user) {
    return jsonResponse({ error: "unauthorized", message: "Invalid or expired token" }, 401);
  }
  const user = userData.user;
  const userId = user.id;

  // Load live billing settings
  let paymentLink = safeHttpUrl(Deno.env.get("PAYMENT_LINK"), DEFAULT_PAYMENT_LINK);
  let topupPaused = /^(on|true|1|paused|yes)$/i.test((Deno.env.get("TOPUP_PAUSED") ?? "").trim());
  let paidDailyCredits = 15;
  let freeDailyCredits = 6;

  try {
    const { data: cfg } = await admin
      .from("app_settings")
      .select("payment_link, topup_paused, paid_daily_credits, free_daily_credits")
      .eq("id", 1)
      .maybeSingle();
    if (cfg) {
      paymentLink = safeHttpUrl(cfg.payment_link, paymentLink);
      if (typeof cfg.topup_paused === "boolean") topupPaused = cfg.topup_paused;
      if (typeof cfg.paid_daily_credits === "number") paidDailyCredits = cfg.paid_daily_credits;
      if (typeof cfg.free_daily_credits === "number") freeDailyCredits = cfg.free_daily_credits;
    }
  } catch (_) {
    // Use defaults if not migrated yet.
  }

  const plans = PLANS.map((p) => ({ ...p, daily_credits: paidDailyCredits }));

  if (req.method === "GET") {
    const [{ data: credits, error: creditsErr }, { data: requests, error: reqErr }] = await Promise.all([
      admin.rpc("sync_user_daily_credits", { p_user_id: userId }),
      admin
        .from("payment_requests")
        .select("id, plan_id, amount_myr, reference, note, receipt_path, status, credit_granted, decision_note, created_at, decided_at")
        .eq("user_id", userId)
        .order("created_at", { ascending: false })
        .limit(20),
    ]);

    if (creditsErr) {
      return jsonResponse({ error: "db_error", message: creditsErr.message }, 500);
    }
    if (reqErr) {
      return jsonResponse({ error: "db_error", message: reqErr.message }, 500);
    }

    const rows = Array.isArray(requests) ? requests : [];
    // Mint 300-second signed URLs for up to 10 most recent receipts.
    const enriched = await Promise.all(
      rows.map(async (row, idx) => {
        let signedUrl: string | null = null;
        if (idx < 10 && typeof row.receipt_path === "string" && row.receipt_path) {
          const { data: signed } = await admin.storage
            .from("receipts")
            .createSignedUrl(row.receipt_path, 300);
          signedUrl = signed?.signedUrl ?? null;
        }
        return {
          ...row,
          receipt_url: signedUrl,
        };
      }),
    );

    return jsonResponse({
      user: { id: userId, email: user.email ?? "" },
      plans,
      free_daily_credits: freeDailyCredits,
      paid_daily_credits: paidDailyCredits,
      payment_link: paymentLink,
      topup_paused: topupPaused,
      credits: credits ?? null,
      requests: enriched,
    });
  }

  if (req.method === "POST") {
    if (topupPaused) {
      return jsonResponse(
        { error: "checkout_paused", message: "Checkout and receipt uploads are temporarily paused." },
        503,
      );
    }

    // Rate limit: max 5 receipt submissions per minute per account.
    const oneMinAgo = new Date(Date.now() - 60_000).toISOString();
    const { count: recentCount } = await admin
      .from("payment_requests")
      .select("id", { count: "exact", head: true })
      .eq("user_id", userId)
      .gte("created_at", oneMinAgo);

    if ((recentCount ?? 0) >= 5) {
      return jsonResponse(
        { error: "rate_limited", message: "Too many receipt uploads. Please wait a minute and try again." },
        429,
      );
    }

    let form: FormData;
    try {
      form = await req.formData();
    } catch (_) {
      return jsonResponse({ error: "invalid_request", message: "Expected multipart/form-data body." }, 400);
    }

    const planId = String(form.get("plan_id") ?? "").trim();
    const plan = plans.find((p) => p.id === planId);
    if (!plan) {
      return jsonResponse({ error: "invalid_plan", message: "Choose pass-15 or pass-30." }, 400);
    }

    const reference = String(form.get("reference") ?? "").trim();
    if (!reference || reference.length > 120) {
      return jsonResponse(
        { error: "invalid_reference", message: "Transaction reference is required (max 120 characters)." },
        400,
      );
    }

    const rawAmount = String(form.get("amount_myr") ?? "").trim();
    const amountMyr = rawAmount ? Math.round(Number(rawAmount) * 100) / 100 : plan.price_myr;
    if (!Number.isFinite(amountMyr) || amountMyr < plan.price_myr || amountMyr > 100000) {
      return jsonResponse(
        {
          error: "invalid_amount",
          message: `Amount must be at least RM ${plan.price_myr.toFixed(2)} for ${plan.name}.`,
        },
        400,
      );
    }

    const note = String(form.get("note") ?? "").trim().slice(0, 500);
    const file = form.get("receipt");
    if (!(file instanceof File) || file.size === 0) {
      return jsonResponse({ error: "missing_receipt", message: "Please attach your receipt file." }, 400);
    }
    if (file.size > MAX_RECEIPT_BYTES) {
      return jsonResponse({ error: "file_too_large", message: "Receipt file must be 5 MB or smaller." }, 400);
    }

    const mime = (file.type || "").toLowerCase();
    const ext = MIME_TO_EXT[mime];
    if (!ext) {
      return jsonResponse(
        { error: "invalid_file_type", message: "Receipt must be a PNG, JPEG, WEBP, or PDF file." },
        400,
      );
    }

    const bytes = new Uint8Array(await file.arrayBuffer());
    if (!receiptBytesMatchType(bytes, mime)) {
      return jsonResponse(
        { error: "invalid_file_signature", message: "File contents do not match the declared image/PDF format." },
        400,
      );
    }

    const receiptPath = `${userId}/${crypto.randomUUID()}.${ext}`;
    const { error: uploadErr } = await admin.storage
      .from("receipts")
      .upload(receiptPath, bytes, {
        contentType: mime,
        upsert: false,
      });

    if (uploadErr) {
      return jsonResponse({ error: "upload_failed", message: uploadErr.message }, 500);
    }

    const { data: created, error: rpcErr } = await admin.rpc("submit_payment_request", {
      p_user_id: userId,
      p_plan_id: plan.id,
      p_amount_myr: amountMyr,
      p_reference: reference,
      p_note: note,
      p_receipt_path: receiptPath,
    });

    if (rpcErr) {
      // Clean up orphan file if DB validation failed (e.g. duplicate reference or 5-pending cap).
      await admin.storage.from("receipts").remove([receiptPath]).catch(() => {});
      return jsonResponse({ error: "submission_rejected", message: rpcErr.message }, 400);
    }

    // Non-fatal operator notification email via Resend.
    await notifyOperatorEmail({
      payerEmail: user.email ?? "",
      planName: plan.name,
      amountMyr,
      reference,
      note,
      filename: `receipt-${reference.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 40)}.${ext}`,
      bytes,
    });

    return jsonResponse({ ok: true, request: created }, 201);
  }

  return jsonResponse({ error: "method_not_allowed" }, 405);
});
