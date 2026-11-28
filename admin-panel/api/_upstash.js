// Lightweight Upstash Redis REST helper for Admin Panel Vercel API routes.
// Uses UPSTASH_REDIS_REST_URL and UPSTASH_REDIS_REST_TOKEN when configured; fails open if unset.

const REDIS_TIMEOUT_MS = 2500;
const ADMIN_MAX_FAILURES = 5;
const ADMIN_LOCKOUT_SECS = 900; // 15 minutes

function getUpstashConfig() {
  const url = String(process.env.UPSTASH_REDIS_REST_URL || "").trim().replace(/\/+$/, "");
  const token = String(process.env.UPSTASH_REDIS_REST_TOKEN || "").trim();
  if (!url || !token) return null;
  return { url, token };
}

export function getRequestIp(req) {
  // Prefer platform-appended headers; never trust client-spoofable cf-connecting-ip
  // or the left-most x-forwarded-for entry.
  const vercelXff = req.headers?.["x-vercel-forwarded-for"];
  if (typeof vercelXff === "string" && vercelXff.trim()) {
    const hops = vercelXff.split(",").map((s) => s.trim()).filter(Boolean);
    if (hops.length > 0) return hops[hops.length - 1];
  }
  const real = req.headers?.["x-real-ip"];
  if (typeof real === "string" && real.trim()) return real.trim();
  const xff = req.headers?.["x-forwarded-for"];
  if (typeof xff === "string" && xff.trim()) {
    const hops = xff.split(",").map((s) => s.trim()).filter(Boolean);
    if (hops.length > 0) return hops[hops.length - 1];
  }
  return req.socket?.remoteAddress || "unknown";
}

async function redisCommand(command) {
  const cfg = getUpstashConfig();
  if (!cfg) return null;
  try {
    const resp = await fetch(cfg.url, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${cfg.token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(command),
      signal: AbortSignal.timeout(REDIS_TIMEOUT_MS),
    });
    if (!resp.ok) return null;
    const json = await resp.json();
    if (json?.error) return null;
    return json?.result ?? null;
  } catch (_) {
    return null;
  }
}

export async function checkAdminLoginLockout(ip) {
  if (!ip || ip === "unknown" || !getUpstashConfig()) return { locked: false, retryAfterSecs: 0 };
  const ttl = await redisCommand(["TTL", `openmi:admin:lock:${ip}`]);
  if (typeof ttl === "number" && ttl > 0) {
    return { locked: true, retryAfterSecs: ttl };
  }
  return { locked: false, retryAfterSecs: 0 };
}

export async function recordAdminLoginFailure(ip) {
  if (!ip || ip === "unknown" || !getUpstashConfig()) return;
  const failKey = `openmi:admin:fail:${ip}`;
  const count = await redisCommand(["INCR", failKey]);
  if (count === 1) {
    await redisCommand(["EXPIRE", failKey, ADMIN_LOCKOUT_SECS]);
  }
  if (typeof count === "number" && count >= ADMIN_MAX_FAILURES) {
    await redisCommand(["SET", `openmi:admin:lock:${ip}`, "1", "EX", ADMIN_LOCKOUT_SECS]);
    await redisCommand(["DEL", failKey]);
  }
}

export async function clearAdminLoginFailures(ip) {
  if (!ip || ip === "unknown" || !getUpstashConfig()) return;
  await redisCommand(["DEL", `openmi:admin:fail:${ip}`, `openmi:admin:lock:${ip}`]);
}

export async function clearUserCooldown(userId) {
  if (!userId || !getUpstashConfig()) return;
  await redisCommand([
    "DEL",
    `openmi:rl:cooldown:${userId}`,
    `openmi:rl:strikes:${userId}`,
    `openmi:rl:conc:${userId}`,
  ]);
}
