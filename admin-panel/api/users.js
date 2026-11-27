import { readBody, requireAdminAuth, sendJson, supabaseFetch } from "./_supabase.js";
import { clearUserCooldown } from "./_upstash.js";

function isCurrentlyBanned(user) {
  if (!user?.banned_until) return false;
  const ts = Date.parse(user.banned_until);
  return Number.isFinite(ts) && ts > Date.now();
}

export default async function handler(req, res) {
  try {
    if (!requireAdminAuth(req, res)) return;
    if (req.method === "GET") {
      const [authData, creditRows] = await Promise.all([
        supabaseFetch("/auth/v1/admin/users?page=1&per_page=200"),
        supabaseFetch(
          "/rest/v1/user_credits?select=user_id,plan,balance,wallet_credit,daily_tokens_used,total_tokens_used,paid_until,has_paid,active_model,pass_expires_at,updated_at",
        ),
      ]);

      const creditMap = new Map();
      if (Array.isArray(creditRows)) {
        for (const row of creditRows) {
          creditMap.set(row.user_id, row);
        }
      }

      const nowMs = Date.now();
      const rawUsers = Array.isArray(authData?.users) ? authData.users : [];
      const users = rawUsers.map((u) => {
        const c = creditMap.get(u.id);
        const paidUntil = c?.paid_until || null;
        const isPaid = Boolean(paidUntil && Date.parse(paidUntil) > nowMs);
        const passActive = Boolean(c?.active_model && c?.pass_expires_at && Date.parse(c.pass_expires_at) > nowMs);
        return {
          id: u.id,
          email: u.email || "(no email)",
          created_at: u.created_at || null,
          last_sign_in_at: u.last_sign_in_at || null,
          banned: isCurrentlyBanned(u),
          plan: isPaid ? "paid" : "free",
          is_paid: isPaid,
          has_paid: Boolean(c?.has_paid),
          paid_until: paidUntil,
          balance: c ? Number(c.balance) : null,
          wallet_credit: c ? Number(c.wallet_credit || 0) : 0,
          daily_tokens_used: c ? Number(c.daily_tokens_used || 0) : 0,
          total_tokens_used: c ? Number(c.total_tokens_used || 0) : 0,
          active_model: passActive ? c.active_model : null,
          pass_expires_at: passActive ? c.pass_expires_at : null,
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
        if (newId) {
          const balance = Math.max(0, Math.floor(Number(body.balance) || 6));
          const plan = String(body.plan || "free").trim() === "paid" ? "paid" : "free";
          await supabaseFetch("/rest/v1/user_credits?on_conflict=user_id", {
            method: "POST",
            headers: { Prefer: "resolution=merge-duplicates" },
            body: JSON.stringify({
              user_id: newId,
              plan,
              balance,
              wallet_credit: 0,
              updated_at: new Date().toISOString(),
            }),
          });
          if (plan === "paid") {
            await supabaseFetch("/rest/v1/rpc/admin_set_pass", {
              method: "POST",
              body: JSON.stringify({ p_user_id: newId, p_days: 30, p_note: "Created as Paid", p_actor: "admin-panel" }),
            });
          }
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
        if (!banned) await clearUserCooldown(userId);
        return sendJson(res, 200, { ok: true, user_id: userId, banned });
      }

      if (body.action === "clear_cooldown") {
        await clearUserCooldown(userId);
        return sendJson(res, 200, { ok: true, user_id: userId, cleared: true });
      }

      if (body.action === "grant_wallet") {
        const delta = Math.floor(Number(body.delta) || 0);
        const result = await supabaseFetch("/rest/v1/rpc/admin_grant_credit", {
          method: "POST",
          body: JSON.stringify({
            p_user_id: userId,
            p_wallet_delta: delta,
            p_note: String(body.note || "Admin wallet adjustment").trim(),
            p_actor: "admin-panel",
          }),
        });
        return sendJson(res, 200, { ok: true, user_credits: result });
      }

      if (body.action === "set_pass") {
        const unit = String(body.duration_unit || "days").trim().toLowerCase();
        const mode = String(body.mode || "add").trim().toLowerCase();
        const hasCustom = body.duration_value !== undefined || body.paid_until !== undefined || unit !== "days" || mode === "set";

        if (!hasCustom) {
          const days = Math.floor(Number(body.days) || 0);
          const result = await supabaseFetch("/rest/v1/rpc/admin_set_pass", {
            method: "POST",
            body: JSON.stringify({
              p_user_id: userId,
              p_days: days,
              p_note: String(body.note || "").trim(),
              p_actor: "admin-panel",
            }),
          });
          return sendJson(res, 200, { ok: true, user_credits: result });
        }

        // Custom duration (hours, days, weeks, months) or exact paid_until timestamp
        const rawVal = Number(body.duration_value ?? body.days ?? 0);
        if (!body.paid_until && (!Number.isFinite(rawVal) || rawVal <= 0)) {
          // Revoke pass
          const result = await supabaseFetch("/rest/v1/rpc/admin_set_pass", {
            method: "POST",
            body: JSON.stringify({
              p_user_id: userId,
              p_days: 0,
              p_note: String(body.note || "Pass revoked by admin").trim(),
              p_actor: "admin-panel",
            }),
          });
          return sendJson(res, 200, { ok: true, user_credits: result });
        }

        // Fetch current user_credits row to stack on active paid_until when mode === "add"
        const existingRows = await supabaseFetch(
          `/rest/v1/user_credits?user_id=eq.${encodeURIComponent(userId)}&select=balance,wallet_credit,paid_until`,
        );
        const existing = Array.isArray(existingRows) && existingRows[0] ? existingRows[0] : null;
        const nowMs = Date.now();
        const currentUntilMs = existing?.paid_until ? Date.parse(existing.paid_until) : 0;
        const baseMs = mode === "add" && Number.isFinite(currentUntilMs) && currentUntilMs > nowMs ? currentUntilMs : nowMs;

        let targetIso;
        let label;
        if (body.paid_until) {
          const parsed = Date.parse(String(body.paid_until));
          if (!Number.isFinite(parsed) || parsed <= nowMs) {
            return sendJson(res, 400, { error: "paid_until must be a valid future date/time" });
          }
          targetIso = new Date(parsed).toISOString();
          label = `until ${targetIso.slice(0, 16).replace("T", " ")} UTC`;
        } else {
          const unitMs =
            unit === "hours" ? 3600_000 :
            unit === "weeks" ? 7 * 86400_000 :
            unit === "months" ? 30 * 86400_000 :
            86400_000;
          targetIso = new Date(baseMs + Math.round(rawVal * unitMs)).toISOString();
          label = `${mode === "add" ? "+" : ""}${rawVal} ${unit}`;
        }

        const nextBalance = Math.max(Number(existing?.balance ?? 0), 15);
        await supabaseFetch("/rest/v1/user_credits?on_conflict=user_id", {
          method: "POST",
          headers: { Prefer: "resolution=merge-duplicates" },
          body: JSON.stringify({
            user_id: userId,
            plan: "paid",
            paid_until: targetIso,
            balance: nextBalance,
            wallet_credit: Number(existing?.wallet_credit ?? 0),
            updated_at: new Date().toISOString(),
          }),
        });

        await supabaseFetch("/rest/v1/credit_ledger", {
          method: "POST",
          body: JSON.stringify({
            user_id: userId,
            kind: "admin_pass",
            daily_delta: 0,
            wallet_delta: 0,
            reference: "admin-panel",
            note: String(body.note || `Admin granted Paid pass (${label})`).trim(),
          }),
        }).catch(() => {});

        return sendJson(res, 200, { ok: true, paid_until: targetIso, label });
      }

      if (body.action === "update_credits") {
        const balance = Number(body.balance);
        const wallet = body.wallet_credit !== undefined ? Number(body.wallet_credit) : undefined;
        if (!Number.isFinite(balance) || balance < 0) {
          return sendJson(res, 400, { error: "balance must be a non-negative number" });
        }
        const payload = {
          user_id: userId,
          balance: Math.floor(balance),
          updated_at: new Date().toISOString(),
        };
        if (wallet !== undefined && Number.isFinite(wallet) && wallet >= 0) {
          payload.wallet_credit = Math.floor(wallet);
        }
        const row = await supabaseFetch("/rest/v1/user_credits?on_conflict=user_id", {
          method: "POST",
          headers: { Prefer: "return=representation,resolution=merge-duplicates" },
          body: JSON.stringify(payload),
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
