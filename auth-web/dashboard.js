import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";
const CREDITS_ENDPOINT = `${SUPABASE_URL}/functions/v1/credits`;
const MODELS_ENDPOINT = `${SUPABASE_URL}/functions/v1/models`;
const REDEEM_ENDPOINT = `${SUPABASE_URL}/functions/v1/redeem`;
const REFERRALS_ENDPOINT = `${SUPABASE_URL}/functions/v1/referrals`;
const REF_STORAGE_KEY = "openmi_pending_ref_code";

// Preserve legacy `/?code=XXXX-XXXX` links by forwarding directly to `/verify`.
const initialParams = new URLSearchParams(window.location.search);
if (initialParams.get("code")) {
  window.location.replace(`/verify${window.location.search}`);
}
const initialRef = (initialParams.get("ref") || "").trim().toUpperCase();
if (initialRef) {
  try {
    localStorage.setItem(REF_STORAGE_KEY, initialRef);
  } catch (_) {}
}

const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});

const $ = (id) => document.getElementById(id);

let currentSession = null;
let currentCredits = null;
let loadedModels = [];

function escapeHtml(str) {
  return String(str ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function fmtTokens(n) {
  const v = Math.max(0, Number(n) || 0);
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
  if (v >= 1_000) return `${(v / 1_000).toFixed(1)}k`;
  return String(Math.floor(v));
}

function fmtContext(n) {
  const v = Number(n) || 128000;
  return v >= 1000 ? `${Math.round(v / 1000)}k` : String(v);
}

function fmtDate(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

function fmtDateTime(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function fmtCountdown(iso) {
  if (!iso) return "";
  const ms = Date.parse(iso) - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return "now";
  const totalMins = Math.ceil(ms / 60000);
  const h = Math.floor(totalMins / 60);
  const m = totalMins % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function setError(msg) {
  const el = $("global-error");
  if (!msg) {
    el.classList.add("hidden");
    el.textContent = "";
    return;
  }
  el.textContent = msg;
  el.classList.remove("hidden");
}

function renderServiceStatus(aiEnabled) {
  const chip = $("service-status-chip");
  const label = $("service-status-label");
  chip.classList.remove("is-online", "is-paused");
  if (aiEnabled === false) {
    chip.classList.add("is-paused");
    label.textContent = "AI Service: Paused";
  } else {
    chip.classList.add("is-online");
    label.textContent = "AI Service: Operational";
  }
}

function renderCredits(c) {
  currentCredits = c;
  const isPaid = Boolean(c.is_paid);
  const dailyBal = Number(c.balance ?? 0);
  const dailyLimit = Number(c.daily_limit ?? (isPaid ? 15 : 6));
  const wallet = Number(c.wallet_credit ?? 0);
  const tokensUsed = Number(c.daily_tokens_used ?? 0);
  const tokenLimit = Number(c.daily_token_limit ?? (isPaid ? 7_000_000 : 2_000_000));
  const totalTokens = Number(c.total_tokens_used ?? tokensUsed);
  const maxSteps = Number(c.max_steps ?? (isPaid ? 70 : 40));
  const passMins = Number(c.pass_duration_minutes ?? 60);

  renderServiceStatus(c.ai_enabled !== false);

  // Tier badges & CTA
  const tierText = isPaid ? "PAID" : "FREE";
  const tierClass = isPaid ? "badge badge-paid" : "badge badge-free";
  $("sidebar-tier").textContent = tierText;
  $("sidebar-tier").className = tierClass;
  $("metric-tier-badge").textContent = tierText;
  $("metric-tier-badge").className = tierClass;
  $("btn-upgrade-cta").textContent = isPaid ? "Extend Pass" : "Upgrade Pass";

  const paidNote = isPaid && c.paid_until ? `Paid until ${fmtDate(c.paid_until)}` : "Free tier · 00:00 UTC reset";
  $("sidebar-paid-until").textContent = paidNote;

  // Daily credits
  $("metric-daily-credits").textContent = `${dailyBal} / ${dailyLimit}`;
  const creditPct = dailyLimit > 0 ? Math.min(100, Math.max(0, (dailyBal / dailyLimit) * 100)) : 100;
  const creditBar = $("bar-daily-credits");
  creditBar.style.width = `${creditPct}%`;
  creditBar.className = `progress-fill${creditPct === 0 ? " is-danger" : creditPct <= 25 ? " is-warn" : ""}`;
  const resetIn = fmtCountdown(c.next_reset_at);
  $("metric-reset-countdown").textContent = resetIn
    ? `Resets in ${resetIn} (00:00 UTC)`
    : "Resets at 00:00 UTC";

  // Daily tokens
  if (tokenLimit > 0) {
    const pct = Math.min(100, Math.round((tokensUsed / tokenLimit) * 100));
    $("metric-daily-tokens").textContent = `${fmtTokens(tokensUsed)} / ${fmtTokens(tokenLimit)}`;
    $("metric-token-pct").textContent = `${pct}%`;
    const tokBar = $("bar-daily-tokens");
    tokBar.style.width = `${pct}%`;
    tokBar.className = `progress-fill${pct >= 100 ? " is-danger" : pct >= 80 ? " is-warn" : ""}`;
    $("metric-token-sub").textContent =
      pct >= 100
        ? "Daily hard cap reached — resets at 00:00 UTC"
        : `${fmtTokens(Math.max(0, tokenLimit - tokensUsed))} remaining today (hard cap)`;
  } else {
    $("metric-daily-tokens").textContent = fmtTokens(tokensUsed);
    $("metric-token-pct").textContent = "∞";
    $("bar-daily-tokens").style.width = "0%";
    $("metric-token-sub").textContent = "Unlimited daily token cap";
  }

  // Lifetime tokens (from account creation until now)
  $("metric-lifetime-tokens").textContent = totalTokens.toLocaleString("en-US");
  const createdAt = currentSession?.user?.created_at;
  $("metric-lifetime-sub").textContent = createdAt
    ? `In + out since ${fmtDate(createdAt)}`
    : "Total input + output since account creation";

  // Wallet & turns
  $("metric-max-steps").textContent = `${maxSteps} turns`;
  $("metric-pass-duration").textContent = `${passMins}m pass`;
  $("metric-wallet-line").textContent = `Wallet: ${wallet} ${wallet === 1 ? "Coin" : "Coins"} (no expiry · 1:1) · ${paidNote}`;

  // Active hourly pass
  const activeMs = c.pass_expires_at ? Date.parse(c.pass_expires_at) - Date.now() : 0;
  const badge = $("pass-status-badge");
  const desc = $("pass-status-desc");
  if (c.active_model && Number.isFinite(activeMs) && activeMs > 0) {
    const minsLeft = Math.ceil(activeMs / 60000);
    badge.textContent = `Active · ${minsLeft}m left`;
    badge.className = "badge badge-paid";
    desc.innerHTML = `Unlimited prompts on <span class="mono" style="font-weight:600;">${escapeHtml(c.active_model)}</span> until ${escapeHtml(fmtDateTime(c.pass_expires_at))} (subject to daily token cap).`;
  } else {
    badge.textContent = "Idle";
    badge.className = "badge badge-free";
    desc.textContent = `No active hourly pass. Your ${passMins}-minute pass starts automatically on your first CLI prompt.`;
  }

  renderModels();
}

function renderModels() {
  const tbody = $("models-tbody");
  const q = ($("model-search").value || "").trim().toLowerCase();
  const isPaid = Boolean(currentCredits?.is_paid);
  const aiOnline = currentCredits ? currentCredits.ai_enabled !== false : true;
  const activeModel =
    currentCredits?.active_model &&
    currentCredits?.pass_expires_at &&
    Date.parse(currentCredits.pass_expires_at) > Date.now()
      ? currentCredits.active_model
      : null;

  const list = loadedModels.filter((m) => {
    if (!q) return true;
    const hay = `${m.id} ${m.display_name || ""} ${m.label || ""} ${m.description || ""}`.toLowerCase();
    return hay.includes(q);
  });

  if (list.length === 0) {
    tbody.innerHTML = `<tr><td colspan="4" class="empty-cell">No matching models.</td></tr>`;
    return;
  }

  tbody.innerHTML = list
    .map((m) => {
      const name = m.display_name || m.label || m.id;
      const cost = Number(m.credit_cost ?? 3);
      let availHtml = "";
      if (!aiOnline) {
        availHtml = `<span class="badge badge-danger">Service Paused</span>`;
      } else if (activeModel && activeModel === m.id) {
        availHtml = `<span class="badge badge-paid">Active Pass</span>`;
      } else if (m.paid_only && !isPaid) {
        availHtml = `<span class="badge badge-warn">Paid Only</span> <a href="/pay" class="link-btn" style="margin-left:6px;">Unlock</a>`;
      } else {
        availHtml = `<span class="badge badge-paid">Available</span>`;
      }

      return `
        <tr>
          <td>
            <div class="model-name">${escapeHtml(name)}</div>
            <div class="model-id mono">${escapeHtml(m.id)}</div>
            ${m.description ? `<div class="model-desc">${escapeHtml(m.description)}</div>` : ""}
          </td>
          <td class="mono">${escapeHtml(fmtContext(m.context_window))}</td>
          <td class="mono">${cost} cr/hr</td>
          <td>${availHtml}</td>
        </tr>
      `;
    })
    .join("");
}

function renderLedger(rows) {
  const tbody = $("ledger-tbody");
  if (!Array.isArray(rows) || rows.length === 0) {
    tbody.innerHTML = `<tr><td colspan="5" class="empty-cell">No credit activity recorded yet.</td></tr>`;
    return;
  }

  const kindLabels = {
    hour_charge: { label: "Hour Pass", cls: "badge-free" },
    hour_refund: { label: "Refund", cls: "badge-paid" },
    topup: { label: "Pass Top-up", cls: "badge-paid" },
    admin_grant: { label: "Credit Grant", cls: "badge-paid" },
    admin_pass: { label: "Pass Update", cls: "badge-paid" },
    redeem_code: { label: "Coin Code", cls: "badge-paid" },
    referral_bonus: { label: "Invite Bonus", cls: "badge-paid" },
  };

  const fmtDelta = (n) => {
    const v = Number(n) || 0;
    if (v > 0) return `<span style="color:var(--ok);font-weight:600;">+${v}</span>`;
    if (v < 0) return `<span style="color:var(--ink-secondary);">${v}</span>`;
    return `<span style="color:var(--ink-muted);">0</span>`;
  };

  tbody.innerHTML = rows
    .map((r) => {
      const k = kindLabels[r.kind] || { label: r.kind || "Event", cls: "badge-free" };
      return `
        <tr>
          <td class="mono" style="white-space:nowrap;">${escapeHtml(fmtDateTime(r.created_at))}</td>
          <td><span class="badge ${k.cls}">${escapeHtml(k.label)}</span></td>
          <td class="mono">${fmtDelta(r.daily_delta)}</td>
          <td class="mono">${fmtDelta(r.wallet_delta)}</td>
          <td>${escapeHtml(r.note || r.reference || "—")}</td>
        </tr>
      `;
    })
    .join("");
}

async function loadModels() {
  try {
    const resp = await fetch(MODELS_ENDPOINT);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const data = await resp.json();
    loadedModels = Array.isArray(data) ? data : [];
    renderModels();
  } catch (err) {
    $("models-tbody").innerHTML = `<tr><td colspan="4" class="empty-cell">Could not load models (${escapeHtml(err.message)}).</td></tr>`;
  }
}

async function loadDashboard() {
  setError("");
  await loadModels();

  if (!currentSession?.access_token) {
    renderServiceStatus(true);
    return;
  }

  try {
    const [creditsResp, ledgerRes] = await Promise.all([
      fetch(CREDITS_ENDPOINT, {
        headers: { Authorization: `Bearer ${currentSession.access_token}` },
      }),
      supabase
        .from("credit_ledger")
        .select("kind, daily_delta, wallet_delta, reference, note, created_at")
        .order("created_at", { ascending: false })
        .limit(20),
    ]);

    const creditsData = await creditsResp.json().catch(() => ({}));
    if (!creditsResp.ok) {
      throw new Error(creditsData.message || creditsData.error || `HTTP ${creditsResp.status}`);
    }

    renderCredits(creditsData);
    renderLedger(ledgerRes.error ? [] : ledgerRes.data || []);
    await loadReferrals();
  } catch (err) {
    setError(`Failed to load account status: ${err.message}`);
  }
}

let currentReferralCode = "";

function renderReferrals(r) {
  currentReferralCode = r.code || "";
  const inviterCoins = Number(r.inviter_coins ?? 3);
  const inviteeCoins = Number(r.invitee_coins ?? 3);
  const stats = r.stats || {};

  $("referral-code-text").textContent = currentReferralCode || "—";
  $("referral-earned-badge").textContent = `${stats.coins_earned ?? 0} Coins earned`;
  $("referral-rule-desc").textContent = r.referral_enabled === false
    ? "The referral program is currently paused."
    : `Share your link or code. You earn +${inviterCoins} Coins and your friend earns +${inviteeCoins} Coins once they upgrade to a Paid pass for the first time.`;
  $("ref-stat-total").textContent = String(stats.total_invited ?? 0);
  $("ref-stat-pending").textContent = String(stats.pending_count ?? 0);
  $("ref-stat-rewarded").textContent = String(stats.rewarded_count ?? 0);

  const statusEl = $("referral-referred-status");
  const applyForm = $("apply-ref-form");
  if (r.referred_by) {
    applyForm.classList.add("hidden");
    statusEl.classList.remove("hidden");
    if (r.referred_by.status === "rewarded") {
      statusEl.textContent = `Referred via ${r.referred_by.code} · +${r.referred_by.invitee_coins_granted} Coins welcome bonus granted!`;
    } else {
      statusEl.textContent = `Referred via ${r.referred_by.code} · Pending: upgrade to a Paid pass once to unlock +${inviteeCoins} Coins for you and +${inviterCoins} Coins for your inviter.`;
    }
  } else if (r.referral_enabled !== false && !currentCredits?.is_paid) {
    statusEl.classList.add("hidden");
    applyForm.classList.remove("hidden");
  } else {
    statusEl.classList.add("hidden");
    applyForm.classList.add("hidden");
  }
}

async function loadReferrals() {
  if (!currentSession?.access_token) return;
  try {
    // Auto-apply any pending ?ref=CODE captured before sign-in
    let pendingRef = "";
    try {
      pendingRef = (localStorage.getItem(REF_STORAGE_KEY) || "").trim().toUpperCase();
    } catch (_) {}
    if (pendingRef) {
      try {
        localStorage.removeItem(REF_STORAGE_KEY);
      } catch (_) {}
      await fetch(REFERRALS_ENDPOINT, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${currentSession.access_token}`,
        },
        body: JSON.stringify({ code: pendingRef }),
      }).catch(() => {});
    }

    const resp = await fetch(REFERRALS_ENDPOINT, {
      headers: { Authorization: `Bearer ${currentSession.access_token}` },
    });
    if (!resp.ok) return;
    const data = await resp.json();
    renderReferrals(data);
  } catch (_) {
    // non-fatal
  }
}

function applyAuth(session) {
  currentSession = session;
  const signedIn = Boolean(session?.user);

  $("sidebar-account").classList.toggle("hidden", !signedIn);
  $("sidebar-signin-btn").classList.toggle("hidden", signedIn);
  $("signed-out-section").classList.toggle("hidden", signedIn);
  $("usage-section").classList.toggle("hidden", !signedIn);
  $("activity").classList.toggle("hidden", !signedIn);

  if (signedIn) {
    $("sidebar-email").textContent = session.user.email || session.user.id;
  } else {
    currentCredits = null;
  }

  loadDashboard();
}

// Event bindings
$("model-search").addEventListener("input", renderModels);

$("btn-refresh").addEventListener("click", async () => {
  const btn = $("btn-refresh");
  btn.disabled = true;
  btn.textContent = "Refreshing…";
  try {
    await loadDashboard();
  } finally {
    btn.disabled = false;
    btn.textContent = "Refresh";
  }
});

$("btn-signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  window.location.reload();
});

$("redeem-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!currentSession?.access_token) return;
  const input = $("redeem-code-input");
  const btn = $("btn-redeem-code");
  const statusEl = $("redeem-status-msg");
  const code = (input.value || "").trim().toUpperCase();
  if (!code) return;

  btn.disabled = true;
  btn.textContent = "Redeeming…";
  statusEl.style.color = "var(--ink-muted)";
  statusEl.textContent = "Checking code…";

  try {
    const resp = await fetch(REDEEM_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${currentSession.access_token}`,
      },
      body: JSON.stringify({ code }),
    });
    const data = await resp.json().catch(() => ({}));
    if (!resp.ok) {
      throw new Error(data.message || data.error || `HTTP ${resp.status}`);
    }
    const parts = [];
    if (data.coins_granted > 0) parts.push(`+${data.coins_granted} Coins`);
    if (data.bonus_pass_days > 0) parts.push(`+${data.bonus_pass_days}d Paid Pass`);
    statusEl.style.color = "var(--ok)";
    statusEl.textContent = `Redeemed ${data.code}: ${parts.join(" & ")} added to your account!`;
    input.value = "";
    await loadDashboard();
  } catch (err) {
    statusEl.style.color = "var(--danger)";
    statusEl.textContent = err.message || "Could not redeem code.";
  } finally {
    btn.disabled = false;
    btn.textContent = "Redeem";
  }
});

$("btn-copy-ref-code")?.addEventListener("click", async (e) => {
  if (!currentReferralCode) return;
  try {
    await navigator.clipboard.writeText(currentReferralCode);
    e.currentTarget.textContent = "Copied";
    setTimeout(() => { e.currentTarget.textContent = "Copy Code"; }, 1400);
  } catch (_) {}
});

$("btn-copy-ref-link")?.addEventListener("click", async (e) => {
  if (!currentReferralCode) return;
  const link = `${window.location.origin}/login?ref=${encodeURIComponent(currentReferralCode)}`;
  try {
    await navigator.clipboard.writeText(link);
    e.currentTarget.textContent = "Copied Link";
    setTimeout(() => { e.currentTarget.textContent = "Copy Invite Link"; }, 1400);
  } catch (_) {}
});

$("apply-ref-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!currentSession?.access_token) return;
  const input = $("apply-ref-input");
  const btn = $("btn-apply-ref");
  const msgEl = $("apply-ref-msg");
  const code = (input.value || "").trim().toUpperCase();
  if (!code) return;

  btn.disabled = true;
  msgEl.style.color = "var(--ink-muted)";
  msgEl.textContent = "Applying…";
  try {
    const resp = await fetch(REFERRALS_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${currentSession.access_token}`,
      },
      body: JSON.stringify({ code }),
    });
    const data = await resp.json().catch(() => ({}));
    if (!resp.ok) throw new Error(data.message || data.error || `HTTP ${resp.status}`);
    msgEl.style.color = "var(--ok)";
    msgEl.textContent = data.message || "Invite code applied!";
    input.value = "";
    await loadReferrals();
  } catch (err) {
    msgEl.style.color = "var(--danger)";
    msgEl.textContent = err.message || "Could not apply invite code.";
  } finally {
    btn.disabled = false;
  }
});

document.querySelectorAll("[data-copy]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const cmd = btn.dataset.copy || "";
    try {
      await navigator.clipboard.writeText(cmd);
      const prev = btn.textContent;
      btn.textContent = "Copied";
      setTimeout(() => {
        btn.textContent = prev;
      }, 1500);
    } catch (_) {
      // ignore clipboard errors
    }
  });
});

// Keep live countdowns fresh every 30s
setInterval(() => {
  if (currentCredits) renderCredits(currentCredits);
}, 30_000);

supabase.auth.onAuthStateChange((_event, session) => {
  applyAuth(session);
});

const { data } = await supabase.auth.getSession();
applyAuth(data.session);
