// Shared BYOK helpers: AES-256-GCM key encryption/decryption, masking,
// SSRF-safe provider base URL validation, and BYOK model ID formatting.

export interface ByokModelEntry {
  id: string;
  display_name?: string;
  context_window?: number;
}

export interface UserProviderRow {
  id: string;
  user_id: string;
  name: string;
  preset: string;
  base_url: string;
  api_key_enc: string;
  api_key_hint: string;
  models: ByokModelEntry[];
  created_at: string;
  updated_at: string;
}

export const BYOK_MODEL_PREFIX = "byok:";

export const PROVIDER_PRESETS: Record<string, { label: string; base_url: string }> = {
  openrouter: { label: "OpenRouter", base_url: "https://openrouter.ai/api/v1" },
  openai: { label: "OpenAI", base_url: "https://api.openai.com/v1" },
  deepseek: { label: "DeepSeek", base_url: "https://api.deepseek.com/v1" },
  groq: { label: "Groq", base_url: "https://api.groq.com/openai/v1" },
  custom: { label: "Custom OpenAI-Compatible", base_url: "" },
};

export function formatByokModelId(providerId: string, modelId: string): string {
  return `${BYOK_MODEL_PREFIX}${providerId}:${modelId}`;
}

export function parseByokModelId(
  raw: string,
): { providerId: string; upstreamModel: string } | null {
  if (!raw.startsWith(BYOK_MODEL_PREFIX)) return null;
  const rest = raw.slice(BYOK_MODEL_PREFIX.length);
  const sep = rest.indexOf(":");
  if (sep <= 0 || sep === rest.length - 1) return null;
  const providerId = rest.slice(0, sep).trim();
  const upstreamModel = rest.slice(sep + 1).trim();
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(providerId)) {
    return null;
  }
  if (!upstreamModel) return null;
  return { providerId, upstreamModel };
}

export function maskApiKey(rawKey: string): string {
  const key = rawKey.trim();
  if (key.length <= 8) return "••••••••";
  const prefix = key.slice(0, Math.min(6, key.length - 4));
  const suffix = key.slice(-4);
  return `${prefix}••••${suffix}`;
}

function isBlockedHost(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/^\[|\]$/g, "");
  if (!host) return true;
  if (
    host === "localhost" ||
    host === "0.0.0.0" ||
    host === "::" ||
    host === "::1" ||
    host.endsWith(".localhost") ||
    host.endsWith(".local") ||
    host.endsWith(".internal")
  ) {
    return true;
  }
  // IPv4 literal checks (loopback, RFC1918, link-local/metadata, CGNAT, unspecified)
  const ipv4 = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (ipv4) {
    const [a, b] = [Number(ipv4[1]), Number(ipv4[2])];
    if (a === 0 || a === 10 || a === 127) return true;
    if (a === 169 && b === 254) return true;
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 100 && b >= 64 && b <= 127) return true;
  }
  // IPv6 private/link-local/loopback literals
  if (host.includes(":")) {
    if (
      host.startsWith("fc") ||
      host.startsWith("fd") ||
      host.startsWith("fe80") ||
      host.startsWith("::ffff:")
    ) {
      return true;
    }
  }
  return false;
}

export function validateProviderBaseUrl(raw: unknown): { ok: true; url: string } | { ok: false; error: string } {
  const trimmed = typeof raw === "string" ? raw.trim() : "";
  if (!trimmed) {
    return { ok: false, error: "Base URL is required." };
  }
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch (_) {
    return { ok: false, error: "Base URL must be a valid HTTPS URL (e.g. https://openrouter.ai/api/v1)." };
  }
  if (parsed.protocol !== "https:") {
    return { ok: false, error: "Base URL must use https://." };
  }
  if (parsed.username || parsed.password) {
    return { ok: false, error: "Base URL must not contain embedded credentials." };
  }
  if (isBlockedHost(parsed.hostname)) {
    return { ok: false, error: "Private, loopback, and internal hostnames are not allowed." };
  }
  const normalized = parsed.toString().replace(/\/+$/, "");
  return { ok: true, url: normalized };
}

export function normalizeByokModels(raw: unknown): ByokModelEntry[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: ByokModelEntry[] = [];
  for (const item of raw) {
    if (out.length >= 30) break;
    if (!item || typeof item !== "object") continue;
    const obj = item as Record<string, unknown>;
    const id = typeof obj.id === "string" ? obj.id.trim().slice(0, 160) : "";
    if (!id || seen.has(id)) continue;
    seen.add(id);
    const displayName =
      typeof obj.display_name === "string" ? obj.display_name.trim().slice(0, 80) : "";
    const ctxRaw = Number(obj.context_window);
    const contextWindow =
      Number.isFinite(ctxRaw) && ctxRaw >= 1000 && ctxRaw <= 10_000_000
        ? Math.floor(ctxRaw)
        : 128_000;
    out.push({
      id,
      ...(displayName ? { display_name: displayName } : {}),
      context_window: contextWindow,
    });
  }
  return out;
}

function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) {
    bin += String.fromCharCode(bytes[i]);
  }
  return btoa(bin);
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
}

async function getAesKey(): Promise<CryptoKey> {
  const secret = (Deno.env.get("BYOK_ENCRYPTION_KEY") ?? "").trim();
  if (!secret) {
    throw new Error(
      "BYOK encryption is not configured on the server (missing BYOK_ENCRYPTION_KEY secret).",
    );
  }
  // Derive a consistent 256-bit AES key via SHA-256 so any high-entropy secret format works.
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secret));
  return await crypto.subtle.importKey("raw", digest, { name: "AES-GCM" }, false, [
    "encrypt",
    "decrypt",
  ]);
}

export async function encryptApiKey(plaintext: string): Promise<string> {
  const key = await getAesKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new TextEncoder().encode(plaintext)),
  );
  return `v1:${bytesToBase64(iv)}:${bytesToBase64(ct)}`;
}

export async function decryptApiKey(encoded: string): Promise<string> {
  const parts = encoded.split(":");
  if (parts.length !== 3 || parts[0] !== "v1") {
    throw new Error("Invalid encrypted API key format.");
  }
  const key = await getAesKey();
  const iv = base64ToBytes(parts[1]);
  const ct = base64ToBytes(parts[2]);
  const plainBuf = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ct);
  return new TextDecoder().decode(plainBuf);
}
