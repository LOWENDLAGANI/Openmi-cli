import crypto from "node:crypto";

const DEFAULT_SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SESSION_COOKIE_NAME = "openmi_admin_session";
const SESSION_TTL_SECONDS = 12 * 60 * 60; // 12 hours

export function getAdminPassphrase() {
  return (process.env.ADMIN_PASSPHRASE || "").trim();
}

function sha256Buf(str) {
  return crypto.createHash("sha256").update(String(str), "utf8").digest();
}

export function verifyPassphrase(candidate, expected) {
  if (!candidate || !expected) return false;
  return crypto.timingSafeEqual(sha256Buf(candidate), sha256Buf(expected));
}

function signTokenPayload(payload, secret) {
  return crypto.createHmac("sha256", secret).update(payload, "utf8").digest("base64url");
}

function parseCookies(req) {
  const header = req.headers?.cookie || "";
  const out = {};
  for (const part of header.split(";")) {
    const idx = part.indexOf("=");
    if (idx <= 0) continue;
    const k = part.slice(0, idx).trim();
    const v = part.slice(idx + 1).trim();
    out[k] = v;
  }
  return out;
}

function isSecureRequest(req) {
  const proto = String(req.headers?.["x-forwarded-proto"] || "").toLowerCase();
  return proto === "https" || process.env.VERCEL === "1" || process.env.NODE_ENV === "production";
}

export function buildSessionCookie(passphrase, req) {
  const expiresMs = Date.now() + SESSION_TTL_SECONDS * 1000;
  const nonce = crypto.randomBytes(16).toString("base64url");
  const payload = `v1.${expiresMs}.${nonce}`;
  const sig = signTokenPayload(payload, passphrase);
  const parts = [
    `${SESSION_COOKIE_NAME}=${payload}.${sig}`,
    "Path=/",
    "HttpOnly",
    "SameSite=Strict",
    `Max-Age=${SESSION_TTL_SECONDS}`,
  ];
  if (isSecureRequest(req)) parts.push("Secure");
  return parts.join("; ");
}

export function buildClearSessionCookie(req) {
  const parts = [
    `${SESSION_COOKIE_NAME}=`,
    "Path=/",
    "HttpOnly",
    "SameSite=Strict",
    "Max-Age=0",
  ];
  if (isSecureRequest(req)) parts.push("Secure");
  return parts.join("; ");
}

export function isAdminAuthenticated(req) {
  const passphrase = getAdminPassphrase();
  if (!passphrase) return false;

  // 1. Check signed HttpOnly session cookie
  const cookies = parseCookies(req);
  const raw = cookies[SESSION_COOKIE_NAME];
  if (raw) {
    const parts = raw.split(".");
    if (parts.length === 4 && parts[0] === "v1") {
      const expiresMs = Number(parts[1]);
      if (Number.isFinite(expiresMs) && expiresMs > Date.now()) {
        const payload = `${parts[0]}.${parts[1]}.${parts[2]}`;
        const expectedSig = signTokenPayload(payload, passphrase);
        const actualSig = parts[3];
        if (
          expectedSig.length === actualSig.length &&
          crypto.timingSafeEqual(Buffer.from(actualSig, "utf8"), Buffer.from(expectedSig, "utf8"))
        ) {
          return true;
        }
      }
    }
  }

  // 2. Optional header auth for automated scripts (X-Admin-Passphrase or Bearer)
  const headerPass = String(req.headers?.["x-admin-passphrase"] || "").trim();
  if (headerPass && verifyPassphrase(headerPass, passphrase)) {
    return true;
  }
  const auth = String(req.headers?.authorization || "").trim();
  if (auth.startsWith("Bearer ")) {
    const token = auth.slice(7).trim();
    if (token && verifyPassphrase(token, passphrase)) {
      return true;
    }
  }

  return false;
}

export function requireAdminAuth(req, res) {
  if (!getAdminPassphrase()) {
    sendJson(res, 503, {
      error: "ADMIN_PASSPHRASE is not configured in Vercel Project Settings.",
      code: "passphrase_not_configured",
    });
    return false;
  }
  if (!isAdminAuthenticated(req)) {
    sendJson(res, 401, {
      error: "Admin passphrase required.",
      code: "unauthorized",
    });
    return false;
  }
  return true;
}

export function getSupabaseConfig() {
  const url = (process.env.SUPABASE_URL || DEFAULT_SUPABASE_URL).replace(/\/+$/, "");
  const serviceRoleKey = (process.env.SUPABASE_SERVICE_ROLE_KEY || "").trim();
  if (!serviceRoleKey) {
    throw new Error(
      "Missing SUPABASE_SERVICE_ROLE_KEY environment variable in Vercel Project Settings.",
    );
  }
  return { url, serviceRoleKey };
}

export async function supabaseFetch(path, options = {}) {
  const { url, serviceRoleKey } = getSupabaseConfig();
  const headers = {
    apikey: serviceRoleKey,
    Authorization: `Bearer ${serviceRoleKey}`,
    "Content-Type": "application/json",
    ...(options.headers || {}),
  };

  const resp = await fetch(`${url}${path}`, {
    ...options,
    headers,
  });

  const text = await resp.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = text;
  }

  if (!resp.ok) {
    const msg =
      (data && typeof data === "object" && (data.message || data.msg || data.error)) ||
      (typeof data === "string" && data) ||
      `Supabase HTTP ${resp.status}`;
    const err = new Error(msg);
    err.status = resp.status;
    throw err;
  }

  return data;
}

export function maskApiKey(key) {
  const trimmed = String(key || "").trim();
  if (!trimmed) return "";
  if (trimmed.length <= 10) return "••••••••";
  return `${trimmed.slice(0, 6)}••••••••${trimmed.slice(-4)}`;
}

export function sendJson(res, status, payload) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json; charset=utf-8");
  res.setHeader("Cache-Control", "no-store");
  res.end(JSON.stringify(payload));
}

export async function readBody(req) {
  if (req.body && typeof req.body === "object") return req.body;
  if (typeof req.body === "string") return req.body ? JSON.parse(req.body) : {};
  const chunks = [];
  let total = 0;
  for await (const chunk of req) {
    total += chunk.length;
    if (total > 1024 * 1024) {
      const err = new Error("Request body too large");
      err.status = 413;
      throw err;
    }
    chunks.push(chunk);
  }
  const raw = Buffer.concat(chunks).toString("utf8");
  return raw ? JSON.parse(raw) : {};
}
