import { createClient } from "https://esm.sh/@supabase/supabase-js@2";
import {
  initCurrency,
  getCurrencyState,
  formatFromMyr,
  mountCountrySelector,
} from "./currency.js";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";
const PAYMENTS_ENDPOINT = `${SUPABASE_URL}/functions/v1/payments`;
const DEFAULT_PAYMENT_LINK = "https://sociabuzz.com/minetallestmc/tribe";

const DEFAULT_PLANS = [
  {
    id: "pass-30",
    name: "30-day pass",
    price_myr: 29,
    days: 30,
    upfront_credit: 3,
    daily_credits: 15,
  },
  {
    id: "pass-15",
    name: "15-day pass",
    price_myr: 14.99,
    days: 15,
    upfront_credit: 2,
    daily_credits: 15,
  },
];

const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});

const $ = (id) => document.getElementById(id);

let currentSession = null;
let loadedPlans = [...DEFAULT_PLANS];
let lastPaymentLink = DEFAULT_PAYMENT_LINK;
let lastPaused = false;

function showStatus(msg, kind = "") {
  const el = $("status-msg");
  if (!msg) {
    el.classList.add("hidden");
    el.textContent = "";
    return;
  }
  el.textContent = msg;
  el.className = `status-msg${kind ? ` is-${kind}` : ""}`;
}

function escapeHtml(s) {
  return String(s ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function fmtDate(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toISOString().slice(0, 10);
}

function daysUntil(iso) {
  if (!iso) return 0;
  const ms = Date.parse(iso) - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return 0;
  return Math.ceil(ms / (1000 * 60 * 60 * 24));
}

function renderPlans(plans, paymentLink, paused) {
  // Always put 30-day pass first (Most popular), then 15-day pass, then Free tier
  const source = Array.isArray(plans) && plans.length > 0 ? plans : DEFAULT_PLANS;
  const ordered = [...source].sort((a, b) => Number(b.days || 0) - Number(a.days || 0));
  loadedPlans = ordered;
  lastPaymentLink = paymentLink;
  lastPaused = paused;

  const currState = getCurrencyState();
  const freePrice = formatFromMyr(0, currState.currency);

  const paidCards = ordered
    .map((p) => {
      const isPopular = p.id === "pass-30" || Number(p.days) >= 30;
      const tag = isPopular ? "Most popular" : "Fixed term";
      const priceInfo = formatFromMyr(p.price_myr, currState.currency);
      const priceNote = priceInfo.isMyr
        ? ""
        : `<p class="plan-price-note">≈ MYR ${Number(p.price_myr).toFixed(2)} · approx. billed via Sociabuzz</p>`;
      const btnCls = isPopular ? "btn btn-primary" : "btn btn-secondary";
      const ctaLabel = isPopular ? `Get ${escapeHtml(p.name)} →` : `Get ${escapeHtml(p.name)}`;
      const cta = paused
        ? `<button type="button" class="btn btn-secondary" disabled>Checkout paused</button>`
        : `<a href="${escapeHtml(paymentLink)}" target="_blank" rel="noopener noreferrer" class="${btnCls}" data-select-plan="${escapeHtml(p.id)}">${ctaLabel}</a>`;

      return `
        <article class="plan-card${isPopular ? " is-popular" : ""}">
          <div class="plan-head">
            <h2 class="plan-title">${escapeHtml(p.name)}</h2>
            <span class="plan-tag">${tag}</span>
          </div>
          <p class="plan-submeta">${p.daily_credits} Credit/day · +${p.upfront_credit} Wallet Coins</p>
          <div class="plan-price-row">
            <span class="plan-price">${escapeHtml(priceInfo.text)}</span>
            <span class="plan-period">/ ${p.days} days</span>
          </div>
          ${priceNote}
          ${cta}
          <div class="plan-divider" aria-hidden="true"></div>
          <ul class="plan-features">
            <li>
              <div class="plan-feature-title">2.5x daily usage allowance</div>
              <div class="plan-feature-desc">${p.daily_credits} Credit/day instead of 6 Credit/day on Free (resets 00:00 UTC).</div>
            </li>
            <li>
              <div class="plan-feature-title">7M tokens/day &amp; 70 turns/prompt</div>
              <div class="plan-feature-desc">3.5x daily token cap and extended agent turns vs 2M tokens &amp; 40 turns on Free.</div>
            </li>
            <li>
              <div class="plan-feature-title">Unlocks all Paid-only models</div>
              <div class="plan-feature-desc">Full access to frontier coding models alongside Free-tier models in Openmi CLI.</div>
            </li>
            <li>
              <div class="plan-feature-title">+${p.upfront_credit} upfront Wallet Coins</div>
              <div class="plan-feature-desc">Added to your append-only ledger immediately on top of your daily allowance.</div>
            </li>
            <li>
              <div class="plan-feature-title">Stackable, never auto-renews</div>
              <div class="plan-feature-desc">One-time ${p.days}-day pass that extends from your current end date.</div>
            </li>
          </ul>
        </article>
      `;
    })
    .join("");

  const freeCard = `
    <article class="plan-card">
      <div class="plan-head">
        <h2 class="plan-title">Free tier</h2>
        <span class="plan-tag">Always free</span>
      </div>
      <p class="plan-submeta">6 Credit/day · 2M tokens/day</p>
      <div class="plan-price-row">
        <span class="plan-price">${escapeHtml(freePrice.text)}</span>
        <span class="plan-period">/ forever</span>
      </div>
      <a href="/" class="btn btn-secondary">Open dashboard</a>
      <div class="plan-divider" aria-hidden="true"></div>
      <ul class="plan-features">
        <li>
          <div class="plan-feature-title">6 Credit daily allowance</div>
          <div class="plan-feature-desc">Resets every day at 00:00 UTC for everyday terminal coding sessions.</div>
        </li>
        <li>
          <div class="plan-feature-title">2M tokens/day &amp; 40 turns/prompt</div>
          <div class="plan-feature-desc">Generous daily hard cap across all Free-tier models.</div>
        </li>
        <li>
          <div class="plan-feature-title">Instant CLI workspace</div>
          <div class="plan-feature-desc">Sign in with openmi login and start coding right away — no card required.</div>
        </li>
      </ul>
    </article>
  `;

  const grid = $("plans-grid");
  grid.innerHTML = paidCards + freeCard;

  // Keep receipt form's plan dropdown labels synced with the selected currency
  const planSelect = $("plan-select");
  if (planSelect) {
    const prevVal = planSelect.value;
    planSelect.innerHTML = ordered
      .map((p) => {
        const info = formatFromMyr(p.price_myr, currState.currency);
        const priceLabel = info.isMyr
          ? `MYR ${Number(p.price_myr).toFixed(2)}`
          : `≈ ${info.text} · MYR ${Number(p.price_myr).toFixed(2)}`;
        return `<option value="${escapeHtml(p.id)}">${escapeHtml(p.name)} (${escapeHtml(priceLabel)} · +${p.upfront_credit} Wallet Coins)</option>`;
      })
      .join("");
    if (prevVal && ordered.some((p) => p.id === prevVal)) {
      planSelect.value = prevVal;
    }
  }

  // Clicking a pass button pre-selects that plan and opens the 3-second Sociabuzz confirmation modal
  grid.querySelectorAll("[data-select-plan]").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      const planId = el.getAttribute("data-select-plan");
      const select = $("plan-select");
      if (select && planId) {
        select.value = planId;
        syncAmountWithPlan(true);
      }
      const href = el.getAttribute("href");
      if (href) {
        openPayConfirmModal(href);
      }
    });
  });
}

let pendingCheckoutUrl = "";
let payCountdownTimer = null;

function closePayConfirmModal() {
  if (payCountdownTimer) {
    clearInterval(payCountdownTimer);
    payCountdownTimer = null;
  }
  pendingCheckoutUrl = "";
  $("pay-confirm-modal")?.classList.add("hidden");
}

function openPayConfirmModal(url) {
  const modal = $("pay-confirm-modal");
  const continueBtn = $("pay-modal-continue");
  if (!modal || !continueBtn) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }

  if (payCountdownTimer) {
    clearInterval(payCountdownTimer);
    payCountdownTimer = null;
  }

  pendingCheckoutUrl = url;
  let remaining = 3;
  continueBtn.disabled = true;
  continueBtn.textContent = `Continue (${remaining}s)`;
  modal.classList.remove("hidden");

  payCountdownTimer = setInterval(() => {
    remaining -= 1;
    if (remaining <= 0) {
      clearInterval(payCountdownTimer);
      payCountdownTimer = null;
      continueBtn.disabled = false;
      continueBtn.textContent = "Continue";
    } else {
      continueBtn.textContent = `Continue (${remaining}s)`;
    }
  }, 1000);
}

$("pay-modal-cancel")?.addEventListener("click", closePayConfirmModal);

$("pay-confirm-modal")?.addEventListener("click", (e) => {
  if (e.target === $("pay-confirm-modal")) {
    closePayConfirmModal();
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("pay-confirm-modal")?.classList.contains("hidden")) {
    closePayConfirmModal();
  }
});

$("pay-modal-continue")?.addEventListener("click", () => {
  const url = pendingCheckoutUrl;
  closePayConfirmModal();
  if (url) {
    window.open(url, "_blank", "noopener,noreferrer");
  }
});

function renderRequests(requests) {
  const list = $("requests-list");
  if (!Array.isArray(requests) || requests.length === 0) {
    list.innerHTML = `<p class="muted" style="margin:0;">No receipt submissions yet.</p>`;
    return;
  }
  list.innerHTML = requests
    .map((r) => {
      const pillClass =
        r.status === "approved"
          ? "pill-approved"
          : r.status === "rejected"
            ? "pill-rejected"
            : "pill-pending";
      const receiptLink = r.receipt_url
        ? `<a href="${escapeHtml(r.receipt_url)}" target="_blank" rel="noopener noreferrer" class="link-btn">View receipt</a>`
        : "";
      const noteLine = r.decision_note
        ? `<div class="muted">Operator note: ${escapeHtml(r.decision_note)}</div>`
        : "";
      return `
        <div class="req-row">
          <div>
            <div><strong>${escapeHtml(r.plan_id)}</strong> · <span class="mono">MYR ${Number(r.amount_myr).toFixed(2)}</span> · <span class="mono">${escapeHtml(r.reference)}</span></div>
            <div class="muted">Submitted ${escapeHtml(fmtDate(r.created_at))}${r.credit_granted ? ` · +${r.credit_granted} Coins` : ""}</div>
            ${noteLine}
          </div>
          <div style="display:flex;align-items:center;gap:10px;">
            ${receiptLink}
            <span class="pill ${pillClass}">${escapeHtml(r.status)}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

async function loadPayments() {
  if (!currentSession?.access_token) return;
  showStatus("");
  try {
    const resp = await fetch(PAYMENTS_ENDPOINT, {
      headers: { Authorization: `Bearer ${currentSession.access_token}` },
    });
    const data = await resp.json();
    if (!resp.ok) {
      showStatus(data.message || data.error || "Failed to load billing status.", "error");
      return;
    }

    const c = data.credits || {};
    const isPaid = Boolean(c.is_paid);
    const tierEl = $("stat-tier");
    tierEl.textContent = isPaid ? "PAID" : "FREE";
    tierEl.className = `tier-badge ${isPaid ? "is-paid" : "is-free"}`;

    const paidPill = $("stat-paid-until");
    if (isPaid && c.paid_until) {
      const dLeft = daysUntil(c.paid_until);
      paidPill.textContent = `Active until ${fmtDate(c.paid_until)}${dLeft > 0 ? ` (${dLeft} days left)` : ""}`;
      paidPill.className = "active-pill";
    } else {
      paidPill.textContent = "Free tier · resets 00:00 UTC";
      paidPill.className = "active-pill is-free";
    }

    $("stat-daily").textContent = `${c.balance ?? 0} / ${c.daily_limit ?? 6}`;
    const coins = Number(c.wallet_credit ?? 0);
    $("stat-wallet").textContent = `${coins} ${coins === 1 ? "Coin" : "Coins"}`;

    const paused = Boolean(data.topup_paused);
    $("paused-banner").classList.toggle("hidden", !paused);
    $("receipt-form").classList.toggle("hidden", paused);

    renderPlans(data.plans || DEFAULT_PLANS, data.payment_link || DEFAULT_PAYMENT_LINK, paused);
    renderRequests(data.requests || []);
    syncAmountWithPlan();
  } catch (err) {
    showStatus(err.message || "Network error loading billing data.", "error");
  }
}

function syncAmountWithPlan(force = false) {
  const planId = $("plan-select").value;
  const plan = loadedPlans.find((p) => p.id === planId);
  if (!plan) return;
  const input = $("amount-input");
  input.min = String(plan.price_myr);
  input.placeholder = plan.price_myr.toFixed(2);
  if (force || !input.value || Number(input.value) < plan.price_myr) {
    input.value = plan.price_myr.toFixed(2);
  }
}

$("plan-select").addEventListener("change", () => syncAmountWithPlan(true));

$("receipt-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!currentSession?.access_token) return;
  showStatus("");

  const fileInput = $("receipt-file");
  const file = fileInput.files?.[0];
  if (!file) {
    showStatus("Please attach your receipt file.", "error");
    return;
  }
  if (file.size > 5 * 1024 * 1024) {
    showStatus("Receipt file must be 5 MB or smaller.", "error");
    return;
  }

  const fd = new FormData();
  fd.append("plan_id", $("plan-select").value);
  fd.append("amount_myr", $("amount-input").value.trim());
  fd.append("reference", $("ref-input").value.trim());
  fd.append("note", $("note-input").value.trim());
  fd.append("receipt", file);

  const btn = $("submit-btn");
  btn.disabled = true;
  btn.textContent = "Uploading…";

  try {
    const resp = await fetch(PAYMENTS_ENDPOINT, {
      method: "POST",
      headers: { Authorization: `Bearer ${currentSession.access_token}` },
      body: fd,
    });
    const data = await resp.json();
    if (!resp.ok) {
      showStatus(data.message || data.error || "Upload failed.", "error");
      return;
    }
    $("ref-input").value = "";
    $("note-input").value = "";
    fileInput.value = "";
    showStatus("Receipt submitted! Your pass will activate once approved by the operator.", "ok");
    await loadPayments();
  } catch (err) {
    showStatus(err.message || "Network error uploading receipt.", "error");
  } finally {
    btn.disabled = false;
    btn.textContent = "Submit Receipt for Approval →";
  }
});

$("btn-signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  window.location.reload();
});

function renderAuth(session) {
  currentSession = session;
  if (session?.user) {
    $("signed-out-view").classList.add("hidden");
    $("signed-in-view").classList.remove("hidden");
    $("account-bar").classList.remove("hidden");
    $("user-email").textContent = session.user.email || session.user.id;
    loadPayments();
  } else {
    $("signed-in-view").classList.add("hidden");
    $("account-bar").classList.add("hidden");
    $("signed-out-view").classList.remove("hidden");
    renderPlans(DEFAULT_PLANS, DEFAULT_PAYMENT_LINK, false);
  }
}

// Initialize country/currency detection & mount topbar selector
initCurrency();
mountCountrySelector($("country-selector-slot"), () => {
  renderPlans(loadedPlans, lastPaymentLink, lastPaused);
});

// Render default plans immediately so the page never flashes empty
renderPlans(DEFAULT_PLANS, DEFAULT_PAYMENT_LINK, false);

supabase.auth.onAuthStateChange((_event, session) => {
  renderAuth(session);
});

const { data } = await supabase.auth.getSession();
renderAuth(data.session);
