// Openmi Windows App Waitlist Edge Function
//
// Routes:
//   POST /functions/v1/waitlist -> Register email for Windows App waitlist & send Resend confirmation.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";
import { checkSimpleRateLimit, getClientIp } from "../_shared/upstash.ts";

const SUPABASE_URL = Deno.env.get("SUPABASE_URL")!;
const SERVICE_ROLE_KEY = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!;
const MAX_WAITLIST_BODY_BYTES = 4096;
const WAITLIST_GLOBAL_HOURLY_EMAIL_CAP = 200;

const CORS_HEADERS: Record<string, string> = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
};

function jsonResponse(body: Record<string, unknown>, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      ...CORS_HEADERS,
    },
  });
}

function isValidEmail(email: string): boolean {
  if (!email || email.length > 254) return false;
  return /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email);
}

async function sendWaitlistConfirmationEmail(toEmail: string): Promise<boolean> {
  const apiKey = (Deno.env.get("RESEND_API_KEY") ?? "").trim();
  const fromEmail = (
    Deno.env.get("WAITLIST_FROM_EMAIL") ??
    Deno.env.get("RECEIPT_FROM_EMAIL") ??
    "Openmi <noreply@openmi.space>"
  ).trim();
  if (!apiKey) return false;

  try {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        from: fromEmail,
        to: [toEmail],
        subject: "You're on the Openmi Windows App Waitlist",
        text: [
          "Hi there,",
          "",
          "This confirms you're officially registered for the Openmi Windows App waitlist.",
          "We'll email you as soon as the Windows desktop app is released.",
          "",
          "In the meantime, you can already use Openmi CLI on Windows via PowerShell or npm:",
          "https://www.openmi.space/download",
          "",
          "Need help? Visit https://www.openmi.space/support or reply to support@openmi.space.",
          "",
          "— The Openmi Team",
        ].join("\n"),
        html: `
          <div style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Inter,sans-serif;max-width:540px;margin:0 auto;padding:24px;color:#141417;line-height:1.6;">
            <div style="display:inline-block;background:#141417;color:#FFB5D8;font-weight:700;padding:6px 12px;border-radius:8px;font-size:14px;margin-bottom:16px;">Openmi</div>
            <h2 style="margin:0 0 12px;font-size:20px;color:#141417;">You're on the Windows App Waitlist</h2>
            <p style="margin:0 0 12px;color:#4a4a52;">
              This confirms that <strong>${toEmail.replace(/[<>&"]/g, "")}</strong> is registered for the upcoming <strong>Openmi Windows App</strong>.
            </p>
            <p style="margin:0 0 16px;color:#4a4a52;">
              We'll send you an email as soon as it's released and ready to download.
            </p>
            <p style="margin:0 0 20px;">
              <a href="https://www.openmi.space/download" style="display:inline-block;background:#FFB5D8;color:#141417;text-decoration:none;font-weight:600;padding:10px 16px;border-radius:8px;">Try Openmi CLI Today &rarr;</a>
            </p>
            <p style="margin:0;font-size:12.5px;color:#6e6e77;">
              Questions? Contact <a href="mailto:support@openmi.space" style="color:#9d174d;">support@openmi.space</a> or join our <a href="https://discord.gg/wa8KH3wtW" style="color:#9d174d;">Discord server</a>.
            </p>
          </div>
        `,
      }),
    });
    return res.ok;
  } catch (_) {
    return false;
  }
}

Deno.serve(async (req: Request) => {
  if (req.method === "OPTIONS") {
    return jsonResponse({ ok: true }, 200);
  }

  if (req.method !== "POST") {
    return jsonResponse({ error: "method_not_allowed", message: "Use POST to join the waitlist." }, 405);
  }

  const clientIp = getClientIp(req);
  const ipRl = await checkSimpleRateLimit("waitlist_ip", clientIp, 5, 300);
  if (!ipRl.allowed) {
    return jsonResponse(
      { error: "rate_limited", message: "Too many waitlist requests from your network. Please try again in a few minutes." },
      429,
    );
  }

  const rawText = await req.text().catch(() => "");
  if (!rawText || rawText.length > MAX_WAITLIST_BODY_BYTES) {
    return jsonResponse({ error: "invalid_payload", message: "Request body is empty or too large." }, 400);
  }

  let body: { email?: unknown; source?: unknown; captcha_token?: unknown };
  try {
    const parsed = JSON.parse(rawText);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return jsonResponse({ error: "invalid_json", message: "Invalid request body." }, 400);
    }
    body = parsed;
  } catch (_) {
    return jsonResponse({ error: "invalid_json", message: "Invalid request body." }, 400);
  }

  const rawEmail = typeof body.email === "string" ? body.email.trim() : "";
  const normalized = rawEmail.toLowerCase();
  const source = typeof body.source === "string"
    ? body.source.trim().replace(/[^a-zA-Z0-9._-]/g, "").slice(0, 40) || "web"
    : "web";

  if (!isValidEmail(normalized)) {
    return jsonResponse({ error: "invalid_email", message: "Please enter a valid email address." }, 400);
  }

  // TODO: Set WAITLIST_TURNSTILE_SECRET in Supabase Secrets and pass `captcha_token`
  // from the frontend form to enforce Cloudflare Turnstile verification.
  const turnstileSecret = (Deno.env.get("WAITLIST_TURNSTILE_SECRET") ?? "").trim();
  if (turnstileSecret) {
    const token = typeof body.captcha_token === "string" ? body.captcha_token.trim() : "";
    if (!token) {
      return jsonResponse({ error: "captcha_required", message: "Please complete the verification challenge." }, 400);
    }
    try {
      const verifyResp = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ secret: turnstileSecret, response: token, remoteip: clientIp }),
      });
      const verifyJson = (await verifyResp.json().catch(() => ({}))) as { success?: boolean };
      if (!verifyJson.success) {
        return jsonResponse({ error: "captcha_failed", message: "Verification failed. Please try again." }, 403);
      }
    } catch (_) {
      return jsonResponse({ error: "captcha_error", message: "Could not verify challenge right now." }, 503);
    }
  }

  const emailRl = await checkSimpleRateLimit("waitlist_email", normalized, 2, 3600);
  if (!emailRl.allowed) {
    return jsonResponse(
      { error: "rate_limited", message: "This email was recently submitted. Please wait before trying again." },
      429,
    );
  }

  const admin = createClient(SUPABASE_URL, SERVICE_ROLE_KEY, {
    auth: { persistSession: false, autoRefreshToken: false },
  });

  const { data: existing, error: fetchErr } = await admin
    .from("windows_waitlist")
    .select("id, email, status")
    .eq("email_normalized", normalized)
    .maybeSingle();

  if (fetchErr) {
    return jsonResponse({ error: "db_error", message: "Could not check waitlist right now. Please try again." }, 500);
  }

  if (existing && existing.status === "active") {
    return jsonResponse({
      ok: true,
      already_registered: true,
      email: existing.email,
      message: "You're already on the Windows App waitlist! We'll notify you as soon as it launches.",
    });
  }

  if (existing && existing.status === "cancelled") {
    const { error: updateErr } = await admin
      .from("windows_waitlist")
      .update({ status: "active", cancelled_at: null, source })
      .eq("id", existing.id);

    if (updateErr) {
      return jsonResponse({ error: "db_error", message: "Could not re-register right now. Please try again." }, 500);
    }
  } else {
    const { error: insertErr } = await admin
      .from("windows_waitlist")
      .insert({ email: rawEmail, source, status: "active" });

    if (insertErr) {
      if (insertErr.code === "23505") {
        return jsonResponse({
          ok: true,
          already_registered: true,
          email: rawEmail,
          message: "You're already on the Windows App waitlist! We'll notify you as soon as it launches.",
        });
      }
      return jsonResponse({ error: "db_error", message: "Could not save your signup right now. Please try again." }, 500);
    }
  }

  // Global hourly email cap to protect Resend quota and sender reputation.
  const globalCap = await checkSimpleRateLimit(
    "waitlist_global_hour",
    "all",
    WAITLIST_GLOBAL_HOURLY_EMAIL_CAP,
    3600,
  );
  const emailSent = globalCap.allowed ? await sendWaitlistConfirmationEmail(rawEmail) : false;

  return jsonResponse({
    ok: true,
    already_registered: false,
    email: rawEmail,
    email_sent: emailSent,
    message: emailSent
      ? "You're on the Windows App waitlist! Check your inbox for a confirmation email."
      : "You're on the Windows App waitlist! We'll notify you as soon as it launches.",
  });
});
