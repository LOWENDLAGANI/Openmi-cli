const DEFAULT_SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";

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
  for await (const chunk of req) chunks.push(chunk);
  const raw = Buffer.concat(chunks).toString("utf8");
  return raw ? JSON.parse(raw) : {};
}
