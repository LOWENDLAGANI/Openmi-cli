const { requireAdmin, readJsonBody, sendJson, supabaseFetch } = require("./_supabase");

module.exports = async function handler(req, res) {
  if (!requireAdmin(req, res)) return;

  try {
    if (req.method === "GET") {
      const rows = await supabaseFetch(
        "/rest/v1/windows_waitlist?select=*&order=created_at.desc&limit=500",
      );
      return sendJson(res, 200, { entries: Array.isArray(rows) ? rows : [] });
    }

    if (req.method === "POST") {
      const body = await readJsonBody(req);
      const id = String(body.id || "").trim();
      const action = String(body.action || "cancel").trim();
      if (!id) {
        return sendJson(res, 400, { error: "Missing waitlist entry id" });
      }

      if (action === "cancel") {
        const updated = await supabaseFetch(
          `/rest/v1/windows_waitlist?id=eq.${encodeURIComponent(id)}`,
          {
            method: "PATCH",
            headers: { Prefer: "return=representation" },
            body: JSON.stringify({
              status: "cancelled",
              cancelled_at: new Date().toISOString(),
            }),
          },
        );
        return sendJson(res, 200, { entry: Array.isArray(updated) ? updated[0] : updated });
      }

      if (action === "reactivate") {
        const updated = await supabaseFetch(
          `/rest/v1/windows_waitlist?id=eq.${encodeURIComponent(id)}`,
          {
            method: "PATCH",
            headers: { Prefer: "return=representation" },
            body: JSON.stringify({
              status: "active",
              cancelled_at: null,
            }),
          },
        );
        return sendJson(res, 200, { entry: Array.isArray(updated) ? updated[0] : updated });
      }

      return sendJson(res, 400, { error: `Unknown action: ${action}` });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message });
  }
};
