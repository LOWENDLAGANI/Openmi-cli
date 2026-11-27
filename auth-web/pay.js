import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";
const PAYMENTS_ENDPOINT = `${SUPABASE_URL}/functions/v1/payments`;

const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});

const $ = (id) => document.getElementById(id);

let currentSession = null;
let loadedPlans = [];

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
  return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function renderPlans(plans, paymentLink, paused) {
  loadedPlans = plans;
  const grid = $("plans-grid");
  grid.innerHTML = plans
    .map((p) => {
      const cta = paused
        ? `<button type="button" class="btn btn-secondary" disabled>Checkout paused</button>`
        : `<a href="${escapeHtml(paymentLink)}" target="_blank" rel="noopener noreferrer" class="btn btn-primary" style="text-align:center;text-decoration:none;">Pay RM ${p.price_myr} on Sociabuzz →</a>`;
      return `
        <div class="plan-card">
          <h3>${escapeHtml(p.name)}</h3>
          <div class="plan-price">RM ${p.price_myr}</div>
          <ul class="plan-perks">
            <li><strong>${p.daily_credits} Credits/day</strong> for ${p.days} days (resets 00:00 UTC)</li>
            <li><strong>7M tokens/day</strong> cap &amp; <strong>70 turns/prompt</strong> (vs 2M / 40 on Free)</li>
            <li><strong>+${p.upfront_credit} wallet Credits</strong> (never expire)</li>
            <li>Unlocks all <strong>Paid-only</strong> models</li>
            <li>Stacks onto any active pass — no auto-renewal</li>
          </ul>
          ${cta}
        </div>
      `;
    })
    .join("");
}

function renderRequests(requests) {
  const list = $("requests-list");
  if (!Array.isArray(requests) || requests.length === 0) {
    list.innerHTML = `<p class="muted">No receipt submissions yet.</p>`;
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
            <div><strong>${escapeHtml(r.plan_id)}</strong> · <span class="mono">RM ${Number(r.amount_myr).toFixed(2)}</span> · <span class="mono">${escapeHtml(r.reference)}</span></div>
            <div class="muted">Submitted ${escapeHtml(fmtDate(r.created_at))}${r.credit_granted ? ` · +${r.credit_granted} wallet cr` : ""}</div>
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
    $("stat-tier").textContent = isPaid ? "PAID" : "FREE";
    $("stat-paid-until").textContent = isPaid && c.paid_until ? `Until ${fmtDate(c.paid_until)}` : "6 cr/day free tier";
    $("stat-daily").textContent = `${c.balance ?? 0} / ${c.daily_limit ?? 6}`;
    $("stat-wallet").textContent = `${c.wallet_credit ?? 0}`;

    const paused = Boolean(data.topup_paused);
    $("paused-banner").classList.toggle("hidden", !paused);
    $("receipt-form").classList.toggle("hidden", paused);

    renderPlans(data.plans || [], data.payment_link || "https://sociabuzz.com/minetallestmc/tribe", paused);
    renderRequests(data.requests || []);
    syncAmountWithPlan();
  } catch (err) {
    showStatus(err.message || "Network error loading billing data.", "error");
  }
}

function syncAmountWithPlan() {
  const planId = $("plan-select").value;
  const plan = loadedPlans.find((p) => p.id === planId);
  if (!plan) return;
  const input = $("amount-input");
  input.min = String(plan.price_myr);
  input.placeholder = plan.price_myr.toFixed(2);
  if (!input.value || Number(input.value) < plan.price_myr) {
    input.value = plan.price_myr.toFixed(2);
  }
}

$("plan-select").addEventListener("change", syncAmountWithPlan);

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
    btn.textContent = "Submit Receipt for Approval";
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
  }
}

supabase.auth.onAuthStateChange((_event, session) => {
  renderAuth(session);
});

const { data } = await supabase.auth.getSession();
renderAuth(data.session);
