import crypto from "node:crypto";
import { requireAdminAuth, readBody, sendJson, supabaseFetch } from "./_supabase.js";

function generateCode() {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const part = (len) => {
    const bytes = crypto.randomBytes(len);
    let out = "";
    for (let i = 0; i < len; i++) {
      out += chars[bytes[i] % chars.length];
    }
    return out;
  };
  return `OPENMI-${part(4)}-${part(4)}`;
}

export default async function handler(req, res) {
  if (!requireAdminAuth(req, res)) return;

  try {
    if (req.method === "GET") {
      const rows = await supabaseFetch(
        "/rest/v1/redeem_codes?select=*&order=created_at.desc&limit=200",
      );
      return sendJson(res, 200, { codes: Array.isArray(rows) ? rows : [] });
    }

    if (req.method === "POST") {
      const body = await readBody(req);
      const action = String(body.action || "create").trim();

      if (action === "create") {
        const rawCode = String(body.code || "").trim().toUpperCase().replace(/[^A-Z0-9_-]/g, "");
        const code = rawCode || generateCode();
        const coins = Math.max(0, Math.floor(Number(body.coins) || 0));
        const bonusPassDays = Math.max(0, Math.floor(Number(body.bonus_pass_days) || 0));
        if (coins === 0 && bonusPassDays === 0) {
          return sendJson(res, 400, { error: "Provide at least 1 Coin or 1 bonus Paid Pass day." });
        }

        const rawMax = body.max_uses;
        const maxUses =
          rawMax === null || rawMax === undefined || String(rawMax).trim() === ""
            ? null
            : Math.max(1, Math.floor(Number(rawMax) || 1));

        const perUserLimit = Math.max(1, Math.floor(Number(body.per_user_limit) || 1));
        const rawTier = String(body.eligible_tier || "all").trim().toLowerCase();
        const eligibleTier = rawTier === "free" || rawTier === "paid" ? rawTier : "all";

        let expiresAt = null;
        if (body.expires_at && String(body.expires_at).trim() !== "") {
          const parsed = Date.parse(String(body.expires_at));
          if (!Number.isFinite(parsed)) {
            return sendJson(res, 400, { error: "Invalid expires_at date/time" });
          }
          expiresAt = new Date(parsed).toISOString();
        }

        const note = String(body.note || "").trim().slice(0, 200);
        const isActive = body.is_active !== false;

        const inserted = await supabaseFetch("/rest/v1/redeem_codes", {
          method: "POST",
          headers: { Prefer: "return=representation" },
          body: JSON.stringify({
            code,
            coins,
            bonus_pass_days: bonusPassDays,
            max_uses: maxUses,
            per_user_limit: perUserLimit,
            eligible_tier: eligibleTier,
            expires_at: expiresAt,
            is_active: isActive,
            note,
            created_by: "admin-panel",
          }),
        });

        const row = Array.isArray(inserted) ? inserted[0] : inserted;
        return sendJson(res, 200, { ok: true, code: row });
      }

      if (action === "toggle") {
        const id = String(body.id || "").trim();
        if (!id) return sendJson(res, 400, { error: "id is required" });
        const isActive = Boolean(body.is_active);
        await supabaseFetch(`/rest/v1/redeem_codes?id=eq.${encodeURIComponent(id)}`, {
          method: "PATCH",
          body: JSON.stringify({ is_active: isActive, updated_at: new Date().toISOString() }),
        });
        return sendJson(res, 200, { ok: true, id, is_active: isActive });
      }

      if (action === "delete") {
        const id = String(body.id || "").trim();
        if (!id) return sendJson(res, 400, { error: "id is required" });
        await supabaseFetch(`/rest/v1/redeem_codes?id=eq.${encodeURIComponent(id)}`, {
          method: "DELETE",
        });
        return sendJson(res, 200, { ok: true, id, deleted: true });
      }

      return sendJson(res, 400, { error: "Unknown action" });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, 500, { error: err.message });
  }
};
