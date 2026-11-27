import {
  buildClearSessionCookie,
  buildSessionCookie,
  getAdminPassphrase,
  isAdminAuthenticated,
  readBody,
  sendJson,
  verifyPassphrase,
} from "./_supabase.js";

const FAIL_DELAY_MS = 350;

export default async function handler(req, res) {
  try {
    const passphrase = getAdminPassphrase();

    if (req.method === "GET") {
      return sendJson(res, 200, {
        configured: Boolean(passphrase),
        authenticated: Boolean(passphrase && isAdminAuthenticated(req)),
      });
    }

    if (req.method === "DELETE") {
      res.setHeader("Set-Cookie", buildClearSessionCookie(req));
      return sendJson(res, 200, { ok: true, authenticated: false });
    }

    if (req.method === "POST") {
      const body = await readBody(req);
      if (body.action === "logout") {
        res.setHeader("Set-Cookie", buildClearSessionCookie(req));
        return sendJson(res, 200, { ok: true, authenticated: false });
      }

      if (!passphrase) {
        return sendJson(res, 503, {
          error: "ADMIN_PASSPHRASE is not configured in Vercel Project Settings.",
          code: "passphrase_not_configured",
        });
      }

      const candidate = String(body.passphrase || "").trim();
      if (!verifyPassphrase(candidate, passphrase)) {
        await new Promise((resolve) => setTimeout(resolve, FAIL_DELAY_MS));
        return sendJson(res, 401, { error: "Invalid passphrase." });
      }

      res.setHeader("Set-Cookie", buildSessionCookie(passphrase, req));
      return sendJson(res, 200, { ok: true, authenticated: true });
    }

    return sendJson(res, 405, { error: "Method not allowed" });
  } catch (err) {
    return sendJson(res, err.status || 500, { error: err.message || "Internal server error" });
  }
}
