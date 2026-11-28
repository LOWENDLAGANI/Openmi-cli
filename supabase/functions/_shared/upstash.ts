// Shared Upstash Redis REST client & atomic Lua rate limiters for Supabase Edge Functions.
// Requires Supabase Secrets:
//   UPSTASH_REDIS_REST_URL
//   UPSTASH_REDIS_REST_TOKEN
//
// Design:
// - Zero external dependencies (plain `fetch` to Upstash REST API with a 2.5s timeout).
// - Fails open on network/config errors so an Upstash outage never breaks legitimate users
//   (Postgres daily token caps and payload limits still apply).
// - Uses atomic Lua scripts (EVAL) so every chat request needs at most 1 Redis round-trip
//   on entry and 1 on stream completion.

const REDIS_TIMEOUT_MS = 2500;
const CONCURRENCY_LEASE_MS = 120_000; // 2 minutes max lease (auto-expires if isolate crashes)
const ABUSE_STRIKE_WINDOW_SEC = 300; // 5 minutes
const ABUSE_STRIKE_THRESHOLD = 30; // 30 rate-limit hits in 5 min -> 5-minute cooldown
const ABUSE_COOLDOWN_SEC = 300;

function getUpstashConfig(): { url: string; token: string } | null {
  const url = (Deno.env.get("UPSTASH_REDIS_REST_URL") ?? "").trim().replace(/\/+$/, "");
  const token = (Deno.env.get("UPSTASH_REDIS_REST_TOKEN") ?? "").trim();
  if (!url || !token) return null;
  return { url, token };
}

export function isUpstashConfigured(): boolean {
  return getUpstashConfig() !== null;
}

export function getClientIp(req: Request): string {
  // Prefer platform-set x-real-ip on Supabase Edge; never trust client-spoofable
  // cf-connecting-ip or the left-most (client-supplied) x-forwarded-for entry.
  const real = req.headers.get("x-real-ip")?.trim();
  if (real) return real;
  const xff = req.headers.get("x-forwarded-for");
  if (xff) {
    const hops = xff.split(",").map((s) => s.trim()).filter(Boolean);
    if (hops.length > 0) return hops[hops.length - 1];
  }
  return "unknown";
}

async function redisCommand<T = unknown>(command: (string | number)[]): Promise<T | null> {
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
    if (!resp.ok) {
      console.warn(`[upstash] HTTP ${resp.status} for ${command[0]}`);
      return null;
    }
    const json = (await resp.json()) as { result?: T; error?: string };
    if (json.error) {
      console.warn(`[upstash] Redis error: ${json.error}`);
      return null;
    }
    return (json.result ?? null) as T | null;
  } catch (err) {
    console.warn(`[upstash] Request failed (failing open): ${err instanceof Error ? err.message : String(err)}`);
    return null;
  }
}

// Atomic Lua script: checks cooldown, concurrency, anti-TOCTOU in-flight token reservation,
// 10s burst, 60s RPM, 60s TPM, per-IP RPM, global RPM, and global daily token cap in 1 round-trip.
const CHAT_ACQUIRE_LUA = `
local cooldownKey   = KEYS[1]
local strikesKey    = KEYS[2]
local concKey       = KEYS[3]
local b10CurrKey    = KEYS[4]
local b10PrevKey    = KEYS[5]
local rpmCurrKey    = KEYS[6]
local rpmPrevKey    = KEYS[7]
local tpmCurrKey    = KEYS[8]
local tpmPrevKey    = KEYS[9]
local ipCurrKey     = KEYS[10]
local ipPrevKey     = KEYS[11]
local globCurrKey   = KEYS[12]
local globPrevKey   = KEYS[13]
local globDayTokKey = KEYS[14]

local nowMs         = tonumber(ARGV[1])
local elapsed10     = tonumber(ARGV[2])
local elapsed60     = tonumber(ARGV[3])
local rpmLimit      = tonumber(ARGV[4])
local burst10Limit  = tonumber(ARGV[5])
local tpmLimit      = tonumber(ARGV[6])
local maxConc       = tonumber(ARGV[7])
local ipRpmLimit    = tonumber(ARGV[8])
local globRpmLimit  = tonumber(ARGV[9])
local globDayLimit  = tonumber(ARGV[10])
local estInputTok   = tonumber(ARGV[11])
local dailyTokUsed  = tonumber(ARGV[12])
local dailyTokLimit = tonumber(ARGV[13])
local leaseMember   = ARGV[14]
local leaseMs       = tonumber(ARGV[15])
local strikeWinSec  = tonumber(ARGV[16])
local strikeThresh  = tonumber(ARGV[17])
local cooldownSec   = tonumber(ARGV[18])
local hasIp         = tonumber(ARGV[19])

local function sliding(currKey, prevKey, winSec, elapsed)
  local curr = tonumber(redis.call("GET", currKey) or "0")
  local prev = tonumber(redis.call("GET", prevKey) or "0")
  local weight = (winSec - elapsed) / winSec
  if weight < 0 then weight = 0 end
  return curr + math.floor(prev * weight)
end

local function record_strike()
  local s = redis.call("INCR", strikesKey)
  if s == 1 then
    redis.call("EXPIRE", strikesKey, strikeWinSec)
  end
  if s >= strikeThresh then
    redis.call("SET", cooldownKey, "1", "EX", cooldownSec)
    redis.call("DEL", strikesKey)
    return cooldownSec
  end
  return 0
end

local reset60 = math.max(1, 60 - elapsed60)

-- 1. Abuse cooldown check
local cdTtl = redis.call("TTL", cooldownKey)
if cdTtl and cdTtl > 0 then
  return {0, "account_cooldown", cdTtl, rpmLimit, 0, cdTtl}
end

-- 2. Prune expired concurrency leases & check concurrency
redis.call("ZREMRANGEBYSCORE", concKey, "-inf", nowMs)
local activeCount = redis.call("ZCARD", concKey)
if maxConc > 0 and activeCount >= maxConc then
  return {0, "concurrency_limit", 3, maxConc, 0, 3}
end

-- 3. Anti-TOCTOU parallel daily token reservation check
if dailyTokLimit > 0 then
  local members = redis.call("ZRANGE", concKey, 0, -1)
  local inFlightTok = 0
  for i = 1, #members do
    local sep = string.find(members[i], ":")
    if sep then
      local tok = tonumber(string.sub(members[i], sep + 1)) or 0
      inFlightTok = inFlightTok + tok
    end
  end
  if (dailyTokUsed + inFlightTok + estInputTok) > dailyTokLimit then
    return {0, "daily_token_limit_exceeded", 60, dailyTokLimit, 0, 60}
  end
end

-- 4. Burst 10s check
if burst10Limit > 0 then
  local b10 = sliding(b10CurrKey, b10PrevKey, 10, elapsed10)
  if b10 >= burst10Limit then
    local cd = record_strike()
    if cd > 0 then
      return {0, "account_cooldown", cd, rpmLimit, 0, cd}
    end
    local retry = math.max(1, 10 - elapsed10)
    return {0, "rate_limited_burst", retry, burst10Limit, 0, retry}
  end
end

-- 5. User RPM 60s check
local userRpm = sliding(rpmCurrKey, rpmPrevKey, 60, elapsed60)
if rpmLimit > 0 and userRpm >= rpmLimit then
  local cd = record_strike()
  if cd > 0 then
    return {0, "account_cooldown", cd, rpmLimit, 0, cd}
  end
  local retry = math.max(2, math.min(15, reset60))
  return {0, "rate_limited_rpm", retry, rpmLimit, 0, reset60}
end

-- 6. User TPM 60s check
if tpmLimit > 0 then
  local userTpm = sliding(tpmCurrKey, tpmPrevKey, 60, elapsed60)
  if (userTpm + estInputTok) > tpmLimit then
    local cd = record_strike()
    if cd > 0 then
      return {0, "account_cooldown", cd, tpmLimit, 0, cd}
    end
    local retry = math.max(3, math.min(20, reset60))
    return {0, "rate_limited_tpm", retry, tpmLimit, 0, reset60}
  end
end

-- 7. Per-IP RPM 60s check
if hasIp == 1 and ipRpmLimit > 0 then
  local ipRpm = sliding(ipCurrKey, ipPrevKey, 60, elapsed60)
  if ipRpm >= ipRpmLimit then
    local retry = math.max(2, math.min(15, reset60))
    return {0, "rate_limited_ip", retry, ipRpmLimit, 0, reset60}
  end
end

-- 8. Global RPM circuit breaker
if globRpmLimit > 0 then
  local globRpm = sliding(globCurrKey, globPrevKey, 60, elapsed60)
  if globRpm >= globRpmLimit then
    return {0, "global_rate_limited", 5, globRpmLimit, 0, 5}
  end
end

-- 9. Global daily token circuit breaker
if globDayLimit > 0 then
  local globDay = tonumber(redis.call("GET", globDayTokKey) or "0")
  if globDay >= globDayLimit then
    return {0, "global_daily_limit", 60, globDayLimit, 0, 60}
  end
end

-- All checks passed: acquire concurrency lease & increment sliding counters
redis.call("ZADD", concKey, nowMs + leaseMs, leaseMember)
redis.call("EXPIRE", concKey, math.ceil(leaseMs / 1000) + 60)

if burst10Limit > 0 then
  local v = redis.call("INCR", b10CurrKey)
  if v == 1 then redis.call("EXPIRE", b10CurrKey, 25) end
end
if rpmLimit > 0 then
  local v = redis.call("INCR", rpmCurrKey)
  if v == 1 then redis.call("EXPIRE", rpmCurrKey, 130) end
end
if hasIp == 1 and ipRpmLimit > 0 then
  local v = redis.call("INCR", ipCurrKey)
  if v == 1 then redis.call("EXPIRE", ipCurrKey, 130) end
end
if globRpmLimit > 0 then
  local v = redis.call("INCR", globCurrKey)
  if v == 1 then redis.call("EXPIRE", globCurrKey, 130) end
end

local remaining = rpmLimit > 0 and math.max(0, rpmLimit - (userRpm + 1)) or 999999
return {1, "ok", 0, rpmLimit, remaining, reset60}
`;

const CHAT_RELEASE_LUA = `
local concKey       = KEYS[1]
local tpmCurrKey    = KEYS[2]
local globDayTokKey = KEYS[3]

local leaseMember   = ARGV[1]
local actualTokens  = tonumber(ARGV[2]) or 0

redis.call("ZREM", concKey, leaseMember)

if actualTokens > 0 then
  local t = redis.call("INCRBY", tpmCurrKey, actualTokens)
  if t == actualTokens then
    redis.call("EXPIRE", tpmCurrKey, 130)
  end
  local g = redis.call("INCRBY", globDayTokKey, actualTokens)
  if g == actualTokens then
    redis.call("EXPIRE", globDayTokKey, 90000)
  end
end
return 1
`;

const SIMPLE_RL_LUA = `
local currKey = KEYS[1]
local prevKey = KEYS[2]
local winSec  = tonumber(ARGV[1])
local elapsed = tonumber(ARGV[2])
local limit   = tonumber(ARGV[3])

local curr = tonumber(redis.call("GET", currKey) or "0")
local prev = tonumber(redis.call("GET", prevKey) or "0")
local weight = (winSec - elapsed) / winSec
if weight < 0 then weight = 0 end
local count = curr + math.floor(prev * weight)
local reset = math.max(1, winSec - elapsed)

if count >= limit then
  return {0, count, reset}
end

local v = redis.call("INCR", currKey)
if v == 1 then
  redis.call("EXPIRE", currKey, winSec * 2 + 10)
end
return {1, count + 1, reset}
`;

export interface ChatLimitParams {
  userId: string;
  clientIp: string;
  streamId: string;
  rpmLimit: number;
  burst10sLimit: number;
  tpmLimit: number;
  maxConcurrency: number;
  ipRpmLimit: number;
  globalRpmLimit: number;
  globalDailyTokenLimit: number;
  estimatedInputTokens: number;
  dailyTokensUsed: number;
  dailyTokenLimit: number;
}

export interface ChatLimitResult {
  allowed: boolean;
  reason: string;
  retryAfterSecs: number;
  limit: number;
  remaining: number;
  resetSecs: number;
  leaseMember: string;
}

export async function checkAndAcquireChatLimits(params: ChatLimitParams): Promise<ChatLimitResult> {
  const leaseMember = `${params.streamId}:${Math.max(0, Math.floor(params.estimatedInputTokens))}`;
  const defaultOk: ChatLimitResult = {
    allowed: true,
    reason: "ok",
    retryAfterSecs: 0,
    limit: params.rpmLimit,
    remaining: params.rpmLimit,
    resetSecs: 60,
    leaseMember,
  };

  if (!isUpstashConfigured()) return defaultOk;

  const nowMs = Date.now();
  const nowSec = Math.floor(nowMs / 1000);
  const b10Bucket = Math.floor(nowSec / 10);
  const elapsed10 = nowSec % 10;
  const minBucket = Math.floor(nowSec / 60);
  const elapsed60 = nowSec % 60;
  const utcDay = new Date(nowMs).toISOString().slice(0, 10);
  const safeIp = params.clientIp && params.clientIp !== "unknown" ? params.clientIp : "none";
  const hasIp = safeIp !== "none" ? 1 : 0;

  const keys = [
    `openmi:rl:cooldown:${params.userId}`,
    `openmi:rl:strikes:${params.userId}`,
    `openmi:rl:conc:${params.userId}`,
    `openmi:rl:b10:${params.userId}:${b10Bucket}`,
    `openmi:rl:b10:${params.userId}:${b10Bucket - 1}`,
    `openmi:rl:rpm:${params.userId}:${minBucket}`,
    `openmi:rl:rpm:${params.userId}:${minBucket - 1}`,
    `openmi:rl:tpm:${params.userId}:${minBucket}`,
    `openmi:rl:tpm:${params.userId}:${minBucket - 1}`,
    `openmi:rl:ip:${safeIp}:${minBucket}`,
    `openmi:rl:ip:${safeIp}:${minBucket - 1}`,
    `openmi:rl:global:rpm:${minBucket}`,
    `openmi:rl:global:rpm:${minBucket - 1}`,
    `openmi:rl:global:dtok:${utcDay}`,
  ];

  const args = [
    nowMs,
    elapsed10,
    elapsed60,
    Math.max(0, Math.floor(params.rpmLimit)),
    Math.max(0, Math.floor(params.burst10sLimit)),
    Math.max(0, Math.floor(params.tpmLimit)),
    Math.max(1, Math.floor(params.maxConcurrency)),
    Math.max(0, Math.floor(params.ipRpmLimit)),
    Math.max(0, Math.floor(params.globalRpmLimit)),
    Math.max(0, Math.floor(params.globalDailyTokenLimit)),
    Math.max(0, Math.floor(params.estimatedInputTokens)),
    Math.max(0, Math.floor(params.dailyTokensUsed)),
    Math.max(0, Math.floor(params.dailyTokenLimit)),
    leaseMember,
    CONCURRENCY_LEASE_MS,
    ABUSE_STRIKE_WINDOW_SEC,
    ABUSE_STRIKE_THRESHOLD,
    ABUSE_COOLDOWN_SEC,
    hasIp,
  ];

  const res = await redisCommand<[number, string, number, number, number, number]>([
    "EVAL",
    CHAT_ACQUIRE_LUA,
    keys.length,
    ...keys,
    ...args,
  ]);

  if (!Array.isArray(res) || res.length < 6) {
    return defaultOk;
  }

  return {
    allowed: Number(res[0]) === 1,
    reason: String(res[1] ?? "ok"),
    retryAfterSecs: Math.max(0, Number(res[2] ?? 0)),
    limit: Math.max(0, Number(res[3] ?? 0)),
    remaining: Math.max(0, Number(res[4] ?? 0)),
    resetSecs: Math.max(1, Number(res[5] ?? 60)),
    leaseMember,
  };
}

export async function releaseChatSlotAndRecordTokens(
  userId: string,
  leaseMember: string,
  actualTokens: number,
): Promise<void> {
  if (!isUpstashConfigured()) return;
  const nowMs = Date.now();
  const minBucket = Math.floor(nowMs / 60_000);
  const utcDay = new Date(nowMs).toISOString().slice(0, 10);

  const keys = [
    `openmi:rl:conc:${userId}`,
    `openmi:rl:tpm:${userId}:${minBucket}`,
    `openmi:rl:global:dtok:${utcDay}`,
  ];

  await redisCommand([
    "EVAL",
    CHAT_RELEASE_LUA,
    keys.length,
    ...keys,
    leaseMember,
    Math.max(0, Math.floor(actualTokens)),
  ]);
}

export async function checkSimpleRateLimit(
  scope: string,
  key: string,
  limit: number,
  windowSec = 60,
): Promise<{ allowed: boolean; remaining: number; resetSecs: number }> {
  if (!isUpstashConfigured() || limit <= 0) {
    return { allowed: true, remaining: limit, resetSecs: windowSec };
  }
  const nowSec = Math.floor(Date.now() / 1000);
  const bucket = Math.floor(nowSec / windowSec);
  const elapsed = nowSec % windowSec;
  const currKey = `openmi:rl:${scope}:${key}:${bucket}`;
  const prevKey = `openmi:rl:${scope}:${key}:${bucket - 1}`;

  const res = await redisCommand<[number, number, number]>([
    "EVAL",
    SIMPLE_RL_LUA,
    2,
    currKey,
    prevKey,
    windowSec,
    elapsed,
    Math.max(1, Math.floor(limit)),
  ]);

  if (!Array.isArray(res) || res.length < 3) {
    return { allowed: true, remaining: limit, resetSecs: windowSec };
  }

  const allowed = Number(res[0]) === 1;
  const count = Number(res[1] ?? 0);
  const resetSecs = Math.max(1, Number(res[2] ?? windowSec));
  return {
    allowed,
    remaining: Math.max(0, limit - count),
    resetSecs,
  };
}
