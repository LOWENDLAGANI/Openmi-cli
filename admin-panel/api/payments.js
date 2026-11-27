import { getSupabaseConfig, readBody, requireAdminAuth, sendJson, supabaseFetch } from "./_supabase.js";

async function signReceiptUrl(supabaseUrl, receiptPath, expiresIn = 600) {
  if (!receiptPath) return null;
  try {
    const encodedPath = receiptPath
      .split("/")
      .map((seg) => encodeURIComponent(seg))
      .join("/");
    const res = await supabaseFetch(`/storage/v1/object/sign/receipts/${encodedPath}`, {
      method: "POST",
      body: JSON.stringify({ expiresIn }),
    });
    const signed = res?.signedURL || res?.signedUrl;
    if (!signed) return null;
    if (/^https?:\/\//i.test(signed)) return signed;
    return `${supabaseUrl}/storage/v1${signed.startsWith("/") ? "" : "/"}${signed}`;
  } catch (_) {
    return null;
  }
}

export default async function handler(req, res) {
  try {
    if (!requireAdminAuth(req, res)) return;
    const { url: supabaseUrl } = getSupabaseConfig();

    if (req.method === "GET") {
      const [requests, authData, creditRows] = await Promise.all([
        supabaseFetch(
          "/rest/v1/payment_requests?select=*&order=created_at.desc&limit=60",
        ),
        supabaseFetch("/auth/v1/admin/users?page=1&per_page=200").catch(() => ({ users: [] })),
        supabaseFetch(
          "/rest/v1/user_credits?select=user_id,plan,balance,wallet_credit,paid_until",
        ).catch(() => []),
      ]);

      const emailMap = new Map();
      for (const u of Array.isArray(authData?.users) ? authData.users : []) {
        emailMap.set(u.id, u.email || "(no email)");
      }
      const creditMap = new Map();
      for (const c of Array.isArray(creditRows) ? creditRows : []) {
        creditMap.set(c.user_id, c);
      }

      const rawList = Array.isArray(requests) ? requests : [];
      // Sort pending requests first (oldest pending first), then decided newest first.
      const pending = rawList.filter((r) => r.status === "pending").reverse();
      const decided = rawList.filter((r) => r.status !== "pending");
      const ordered = [...pending, ...decided];

      const enriched = await Promise.all(
        ordered.map(async (r, idx) => {
          const c = creditMap.get(r.user_id);
          const receiptUrl =
            r.status === "pending" || idx < 20
              ? await signReceiptUrl(supabaseUrl, r.receipt_path, 600)
              : null;
          return {
            ...r,
            user_email: emailMap.get(r.user_id) || r.user_id,
            user_balance: c ? Number(c.balance) : 0,
            user_wallet_credit: c ? Number(c.wallet_credit || 0) : 0,
            user_paid_until: c?.paid_until || null,
            receipt_url: receiptUrl,
          };
        }),
      );

      return sendJson(res, 200, {
        pending_count: pending.length,
        requests: enriched,
      });
    }

    if (req.method === "POST") {
      const body = await readBody(req);
      if (body.action !== "review") {
        return sendJson(res, 400, { error: "Unknown action" });
      }
      const id = String(body.id || "").trim();
      const decision = String(body.decision || "").trim();
      if (!id || !["approved", "rejected"].includes(decision)) {
        return sendJson(res, 400, { error: "id and decision (approved|rejected) are required" });
      }

      const creditRaw = body.credit_granted;
      const credit =
        creditRaw !== undefined && creditRaw !== null && creditRaw !== ""
          ? Math.min(100000, Math.max(0, Math.floor(Number(creditRaw) || 0)))
          : null;

      const result = await supabaseFetch("/rest/v1/rpc/review_payment_request", {
        method: "POST",
        body: JSON.stringify({
          p_id: id,
          p_decision: decision,
          p_note: String(body.decision_note || "").trim().slice(0, 500),
          p_credit: credit,
          p_decided_by: "admin-panel",
        }),
      });

      return sendJson(res, 200, { ok: true, result });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message || "Internal server error" });
  }
}
