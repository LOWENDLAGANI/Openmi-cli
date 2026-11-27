import { readBody, requireAdminAuth, sendJson, supabaseFetch } from "./_supabase.js";

export default async function handler(req, res) {
  try {
    if (!requireAdminAuth(req, res)) return;
    if (req.method === "GET") {
      const rows = await supabaseFetch(
        "/rest/v1/device_codes?select=user_code,status,user_email,poll_count,created_at,expires_at&order=created_at.desc&limit=50",
      );
      return sendJson(res, 200, { devices: Array.isArray(rows) ? rows : [] });
    }

    if (req.method === "POST") {
      const body = await readBody(req);

      if (body.action === "deny") {
        const userCode = String(body.user_code || "").trim().toUpperCase();
        if (!userCode) return sendJson(res, 400, { error: "user_code is required" });
        await supabaseFetch(`/rest/v1/device_codes?user_code=eq.${encodeURIComponent(userCode)}`, {
          method: "PATCH",
          body: JSON.stringify({ status: "denied" }),
        });
        return sendJson(res, 200, { ok: true, user_code: userCode, status: "denied" });
      }

      if (body.action === "purge_expired") {
        const nowIso = new Date().toISOString();
        await supabaseFetch(
          `/rest/v1/device_codes?or=(expires_at.lt.${encodeURIComponent(nowIso)},status.in.(expired,consumed,denied))`,
          { method: "DELETE" },
        );
        return sendJson(res, 200, { ok: true });
      }

      return sendJson(res, 400, { error: "Unknown action" });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message || "Internal server error" });
  }
}
