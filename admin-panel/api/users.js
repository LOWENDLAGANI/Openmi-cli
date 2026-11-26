import { readBody, sendJson, supabaseFetch } from "./_supabase.js";

function isCurrentlyBanned(user) {
  if (!user?.banned_until) return false;
  const ts = Date.parse(user.banned_until);
  return Number.isFinite(ts) && ts > Date.now();
}

export default async function handler(req, res) {
  try {
    if (req.method === "GET") {
      const [authData, creditRows] = await Promise.all([
        supabaseFetch("/auth/v1/admin/users?page=1&per_page=200"),
        supabaseFetch("/rest/v1/user_credits?select=user_id,plan,balance,updated_at"),
      ]);

      const creditMap = new Map();
      if (Array.isArray(creditRows)) {
        for (const row of creditRows) {
          creditMap.set(row.user_id, row);
        }
      }

      const rawUsers = Array.isArray(authData?.users) ? authData.users : [];
      const users = rawUsers.map((u) => {
        const c = creditMap.get(u.id);
        return {
          id: u.id,
          email: u.email || "(no email)",
          created_at: u.created_at || null,
          last_sign_in_at: u.last_sign_in_at || null,
          banned: isCurrentlyBanned(u),
          plan: c ? String(c.plan) : "free",
          balance: c ? Number(c.balance) : null,
          credits_updated_at: c ? c.updated_at : null,
        };
      });

      return sendJson(res, 200, { users });
    }

    if (req.method === "POST") {
      const body = await readBody(req);

      if (body.action === "create_user") {
        const email = String(body.email || "").trim();
        const password = String(body.password || "");
        if (!email || !password || password.length < 6) {
          return sendJson(res, 400, {
            error: "Valid email and password (at least 6 characters) are required.",
          });
        }
        const created = await supabaseFetch("/auth/v1/admin/users", {
          method: "POST",
          body: JSON.stringify({
            email,
            password,
            email_confirm: true,
          }),
        });
        const newId = created?.id;
        if (newId && (body.balance !== undefined || body.plan)) {
          const balance = Math.max(0, Math.floor(Number(body.balance) || 0));
          const plan = String(body.plan || "free").trim() || "free";
          await supabaseFetch("/rest/v1/user_credits?on_conflict=user_id", {
            method: "POST",
            headers: { Prefer: "resolution=merge-duplicates" },
            body: JSON.stringify({
              user_id: newId,
              plan,
              balance,
              updated_at: new Date().toISOString(),
            }),
          });
        }
        return sendJson(res, 200, { ok: true, id: newId, email });
      }

      const userId = String(body.user_id || "").trim();
      if (!userId) {
        return sendJson(res, 400, { error: "user_id is required" });
      }

      if (body.action === "delete_user") {
        await supabaseFetch(`/auth/v1/admin/users/${encodeURIComponent(userId)}`, {
          method: "DELETE",
        });
        return sendJson(res, 200, { ok: true, user_id: userId, deleted: true });
      }

      if (body.action === "set_ban") {
        const banned = Boolean(body.banned);
        await supabaseFetch(`/auth/v1/admin/users/${encodeURIComponent(userId)}`, {
          method: "PUT",
          body: JSON.stringify({ ban_duration: banned ? "876000h" : "none" }),
        });
        return sendJson(res, 200, { ok: true, user_id: userId, banned });
      }

      if (body.action === "update_credits") {
        const balance = Number(body.balance);
        if (!Number.isFinite(balance) || balance < 0) {
          return sendJson(res, 400, { error: "balance must be a non-negative number" });
        }
        const plan = String(body.plan || "free").trim() || "free";
        const row = await supabaseFetch("/rest/v1/user_credits?on_conflict=user_id", {
          method: "POST",
          headers: { Prefer: "return=representation,resolution=merge-duplicates" },
          body: JSON.stringify({
            user_id: userId,
            plan,
            balance: Math.floor(balance),
            updated_at: new Date().toISOString(),
          }),
        });
        const saved = Array.isArray(row) ? row[0] : row;
        return sendJson(res, 200, { ok: true, user_credits: saved });
      }

      return sendJson(res, 400, { error: "Unknown action" });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message || "Internal server error" });
  }
}
