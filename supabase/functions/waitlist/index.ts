// Openmi Windows App Waitlist Edge Function
//
// Routes:
//   POST /functions/v1/waitlist -> Register email for Windows App waitlist & send Resend confirmation.

import { createClient } from "https://esm.sh/@supabase/supabase-js@2.49.1";

const SUPABASE_URL = Deno.env.get("SUPABASE_URL")!;
const SERVICE_ROLE_KEY = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!;

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

  let body: { email?: string; source?: string };
  try {
    body = await req.json();
  } catch (_) {
    return jsonResponse({ error: "invalid_json", message: "Invalid request body." }, 400);
  }

  const rawEmail = String(body.email ?? "").trim();
  const normalized = rawEmail.toLowerCase();
  const source = String(body.source ?? "web").trim().slice(0, 40) || "web";

  if (!isValidEmail(normalized)) {
    return jsonResponse({ error: "invalid_email", message: "Please enter a valid email address." }, 400);
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
      return jsonResponse({ error: "db_error", message: "Could not save your signup right now. Please try again." }, 500);
    }
  }

  const emailSent = await sendWaitlistConfirmationEmail(rawEmail);

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
