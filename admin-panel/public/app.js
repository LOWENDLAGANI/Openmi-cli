const $ = (id) => document.getElementById(id);

let state = {
  settings: null,
  models: [],
  users: [],
  payments: [],
};

let toastTimer = null;
function showToast(msg, isError = false) {
  const el = $("toast");
  el.textContent = msg;
  el.classList.toggle("is-error", isError);
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove("show"), 3200);
}

function setGlobalError(msg) {
  const el = $("global-error");
  if (!msg) {
    el.classList.add("hidden");
    el.textContent = "";
  } else {
    el.textContent = msg;
    el.classList.remove("hidden");
  }
}

function showAuthGate(message = "") {
  $("app-topbar").classList.add("hidden");
  $("app-main").classList.add("hidden");
  $("auth-gate").classList.remove("hidden");
  const errEl = $("auth-error");
  if (message) {
    errEl.textContent = message;
    errEl.classList.remove("hidden");
  } else {
    errEl.textContent = "";
    errEl.classList.add("hidden");
  }
  const input = $("auth-passphrase");
  if (input) input.focus();
}

function showConsole() {
  $("auth-gate").classList.add("hidden");
  $("app-topbar").classList.remove("hidden");
  $("app-main").classList.remove("hidden");
}

async function api(path, options = {}) {
  const resp = await fetch(path, {
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", ...(options.headers || {}) },
    ...options,
  });
  const data = await resp.json().catch(() => ({}));
  if (resp.status === 401 && path !== "/api/auth") {
    showAuthGate("Session locked or expired. Enter passphrase to continue.");
    throw new Error(data.error || "Unauthorized");
  }
  if (!resp.ok) {
    throw new Error(data.error || `HTTP ${resp.status}`);
  }
  return data;
}

function fmtShortDate(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

function fmtMinsLeft(iso) {
  if (!iso) return "";
  const ms = Date.parse(iso) - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const mins = Math.ceil(ms / 60000);
  return `${mins}m left`;
}

function renderSettings(s) {
  state.settings = s;
  state.models = (s.models || []).map((m) => ({
    ...m,
    credit_cost: typeof m.credit_cost === "number" ? m.credit_cost : 3,
    paid_only: Boolean(m.paid_only),
  }));

  $("project-url").textContent = s.supabase_url || "Supabase";

  const toggle = $("ai-enabled-toggle");
  const wrap = $("status-switch-wrap");
  const label = $("ai-status-label");
  toggle.checked = Boolean(s.ai_enabled);
  wrap.classList.toggle("is-online", Boolean(s.ai_enabled));
  wrap.classList.toggle("is-paused", !s.ai_enabled);
  label.textContent = s.ai_enabled ? "AI Online" : "AI Paused";

  $("metric-ai").textContent = s.ai_enabled ? "Online" : "Paused";
  $("metric-ai").style.color = s.ai_enabled ? "var(--ok)" : "var(--danger)";
  $("metric-key").textContent = s.has_api_key ? s.llm_api_key_masked : "Not set (env fallback)";

  $("llm-base-url").value = s.llm_base_url || "";
  $("llm-default-model").value = s.llm_default_model || "";
  $("free-daily-credits").value = s.free_daily_credits ?? 6;
  $("paid-daily-credits").value = s.paid_daily_credits ?? 15;
  $("free-daily-tokens").value = s.free_daily_token_limit ?? 2000000;
  $("paid-daily-tokens").value = s.paid_daily_token_limit ?? 7000000;
  $("free-max-steps").value = s.free_max_steps ?? 40;
  $("paid-max-steps").value = s.paid_max_steps ?? 70;
  $("pass-duration-minutes").value = s.pass_duration_minutes ?? 60;
  $("credit-cost").value = s.credit_cost ?? 3;
  $("free-rpm").value = s.free_rpm ?? 30;
  $("paid-rpm").value = s.paid_rpm ?? 60;
  $("free-burst-10s").value = s.free_burst_10s ?? 10;
  $("paid-burst-10s").value = s.paid_burst_10s ?? 20;
  $("free-tpm").value = s.free_tpm ?? 400000;
  $("paid-tpm").value = s.paid_tpm ?? 1000000;
  $("free-max-concurrency").value = s.free_max_concurrency ?? 3;
  $("paid-max-concurrency").value = s.paid_max_concurrency ?? 6;
  $("free-max-input-tokens").value = s.free_max_input_tokens ?? 100000;
  $("paid-max-input-tokens").value = s.paid_max_input_tokens ?? 180000;
  $("free-max-output-tokens").value = s.free_max_output_tokens ?? 8192;
  $("paid-max-output-tokens").value = s.paid_max_output_tokens ?? 16384;
  $("ip-chat-rpm").value = s.ip_chat_rpm ?? 90;
  $("global-chat-rpm").value = s.global_chat_rpm ?? 600;
  $("global-daily-token-limit").value = s.global_daily_token_limit ?? 150000000;
  $("payment-link").value = s.payment_link || "https://sociabuzz.com/minetallestmc/tribe";
  $("topup-paused").checked = Boolean(s.topup_paused);
  $("llm-api-key").value = "";
  $("current-key-hint").textContent = s.has_api_key
    ? `Stored: ${s.llm_api_key_masked}`
    : "No DB key stored (falls back to Supabase secret)";

  renderModels();
}

function renderPayments(requests, pendingCount) {
  state.payments = requests || [];
  $("metric-pending").textContent = String(pendingCount ?? 0);
  $("metric-pending").style.color = pendingCount > 0 ? "var(--danger)" : "var(--ok)";

  const tbody = $("payments-tbody");
  if (state.payments.length === 0) {
    tbody.innerHTML = `<tr><td colspan="5" class="empty-cell">No payment receipts submitted yet.</td></tr>`;
    return;
  }

  tbody.innerHTML = "";
  for (const r of state.payments) {
    const defaultCredit = r.plan_id === "pass-30" ? 3 : 2;
    const badgeClass =
      r.status === "approved"
        ? "badge-active"
        : r.status === "rejected"
          ? "badge-banned"
          : "badge-zero";
    const receiptLink = r.receipt_url
      ? `<a href="${escapeAttr(r.receipt_url)}" target="_blank" rel="noopener noreferrer" class="btn btn-sm btn-ghost">Open Receipt ↗</a>`
      : `<span class="hint">Expired link</span>`;

    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td>
        <div class="user-email">${escapeHtml(r.user_email)}</div>
        <div class="user-id">Bal: ${r.user_balance}d + ${r.user_wallet_credit}w${r.user_paid_until ? ` · Paid until ${fmtShortDate(r.user_paid_until)}` : ""}</div>
      </td>
      <td>
        <div><strong>${escapeHtml(r.plan_id)}</strong> · <span class="mono">RM ${Number(r.amount_myr).toFixed(2)}</span></div>
        <div class="hint">${fmtShortDate(r.created_at)}</div>
      </td>
      <td>
        <div class="mono">${escapeHtml(r.reference)}</div>
        ${r.note ? `<div class="hint">${escapeHtml(r.note)}</div>` : ""}
        <div style="margin-top:4px;">${receiptLink}</div>
      </td>
      <td>
        <span class="badge ${badgeClass}">${escapeHtml(r.status)}</span>
        ${r.decision_note ? `<div class="hint">${escapeHtml(r.decision_note)}</div>` : ""}
      </td>
      <td class="col-actions">
        ${
          r.status === "pending"
            ? `<div style="display:flex;flex-direction:column;gap:6px;min-width:190px;">
                 <div style="display:flex;gap:6px;align-items:center;">
                   <input type="number" min="0" max="100000" step="1" class="input mono" style="width:70px;" data-credit value="${defaultCredit}" title="Upfront wallet credits" />
                   <input type="text" class="input" style="flex:1;" data-note placeholder="Note (optional)" />
                 </div>
                 <div class="row-actions">
                   <button type="button" class="btn btn-sm btn-primary" data-approve>Approve</button>
                   <button type="button" class="btn btn-sm btn-danger-ghost" data-reject>Reject</button>
                 </div>
               </div>`
            : `<span class="hint">${r.credit_granted ? `+${r.credit_granted} wallet cr` : "—"}</span>`
        }
      </td>
    `;

    if (r.status === "pending") {
      const creditInput = tr.querySelector("[data-credit]");
      const noteInput = tr.querySelector("[data-note]");
      const approveBtn = tr.querySelector("[data-approve]");
      const rejectBtn = tr.querySelector("[data-reject]");

      const handleDecision = async (decision, btn) => {
        btn.disabled = true;
        try {
          await api("/api/payments", {
            method: "POST",
            body: JSON.stringify({
              action: "review",
              id: r.id,
              decision,
              credit_granted: Number(creditInput.value),
              decision_note: noteInput.value.trim(),
            }),
          });
          showToast(decision === "approved" ? `Approved ${r.plan_id} for ${r.user_email}` : `Rejected receipt ${r.reference}`);
          await loadAll();
        } catch (err) {
          showToast(err.message, true);
          btn.disabled = false;
        }
      };

      approveBtn.addEventListener("click", () => handleDecision("approved", approveBtn));
      rejectBtn.addEventListener("click", () => handleDecision("rejected", rejectBtn));
    }

    tbody.appendChild(tr);
  }
}

function renderModels() {
  const tbody = $("models-tbody");
  tbody.innerHTML = "";
  if (state.models.length === 0) {
    tbody.innerHTML = `<tr><td colspan="9" class="empty-cell">No models configured. Click "+ Add Model".</td></tr>`;
    return;
  }

  state.models.forEach((m, idx) => {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td><input type="text" class="input mono" data-field="id" value="${escapeAttr(m.id)}" placeholder="provider/model-id" /></td>
      <td><input type="text" class="input" data-field="display_name" value="${escapeAttr(m.display_name || m.label || "")}" placeholder="e.g. DeepSeek V3 (Fast)" /></td>
      <td><input type="text" class="input" data-field="description" maxlength="140" value="${escapeAttr(m.description || "")}" placeholder="Short blurb in /model" /></td>
      <td><input type="url" class="input mono" data-field="base_url" value="${escapeAttr(m.base_url || "")}" placeholder="https://api.deepseek.com/v1" autocomplete="off" /></td>
      <td>
        <input type="password" class="input mono" data-field="api_key" value="" autocomplete="new-password"
          placeholder="${m.has_api_key && !m.clear_api_key ? escapeAttr(m.api_key_masked || "saved") + " (saved)" : "blank = global key"}" />
        ${m.has_api_key ? `<button type="button" class="btn btn-sm btn-danger-ghost" data-clear-key="${idx}">${m.clear_api_key ? "Will clear" : "Clear key"}</button>` : ""}
      </td>
      <td><input type="number" min="1000" step="1000" class="input mono" style="width:105px;" data-field="context_window" value="${Number(m.context_window) || 128000}" /></td>
      <td><input type="number" min="0" step="1" class="input mono" style="width:72px;" data-field="credit_cost" value="${Number(m.credit_cost ?? 3)}" /></td>
      <td style="text-align:center;"><input type="checkbox" data-field="paid_only" ${m.paid_only ? "checked" : ""} aria-label="Paid only" /></td>
      <td class="col-actions"><button type="button" class="btn btn-sm btn-danger-ghost" data-remove="${idx}">Remove</button></td>
    `;

    tr.querySelectorAll("input").forEach((inp) => {
      const eventName = inp.type === "checkbox" ? "change" : "input";
      inp.addEventListener(eventName, () => {
        const field = inp.dataset.field;
        if (field === "context_window") {
          state.models[idx][field] = Number(inp.value) || 128000;
        } else if (field === "credit_cost") {
          state.models[idx][field] = Math.max(0, Math.floor(Number(inp.value) || 0));
        } else if (field === "paid_only") {
          state.models[idx][field] = Boolean(inp.checked);
        } else if (field === "display_name") {
          state.models[idx].display_name = inp.value;
          state.models[idx].label = inp.value.trim() || state.models[idx].id;
        } else {
          state.models[idx][field] = inp.value;
        }
      });
    });

    const clearBtn = tr.querySelector("[data-clear-key]");
    if (clearBtn) {
      clearBtn.addEventListener("click", () => {
        state.models[idx].clear_api_key = !state.models[idx].clear_api_key;
        renderModels();
      });
    }

    tr.querySelector(`[data-remove]`).addEventListener("click", () => {
      state.models.splice(idx, 1);
      renderModels();
    });

    tbody.appendChild(tr);
  });
}

function renderUsers() {
  const tbody = $("users-tbody");
  const q = ($("user-search").value || "").trim().toLowerCase();
  const list = state.users.filter(
    (u) => !q || u.email.toLowerCase().includes(q) || u.id.toLowerCase().includes(q),
  );

  $("metric-users").textContent = String(state.users.length);

  if (list.length === 0) {
    tbody.innerHTML = `<tr><td colspan="5" class="empty-cell">${
      state.users.length === 0 ? "No registered users yet." : "No matching users."
    }</td></tr>`;
    return;
  }

  tbody.innerHTML = "";
  for (const u of list) {
    const dailyVal = u.balance === null ? (state.settings?.free_daily_credits ?? 6) : u.balance;
    const walletVal = u.wallet_credit ?? 0;
    const tierBadge = u.banned
      ? `<span class="badge badge-banned">Banned</span>`
      : u.is_paid
        ? `<span class="badge badge-active">PAID</span>`
        : `<span class="badge badge-zero">FREE</span>`;
    const passInfo = u.is_paid && u.paid_until ? `Until ${fmtShortDate(u.paid_until)}` : "No active pass";
    const activeModelInfo = u.active_model
      ? `<div class="mono">${escapeHtml(u.active_model)}</div><div class="hint">${fmtMinsLeft(u.pass_expires_at)}</div>`
      : `<span class="hint">—</span>`;
    const tokenCap = u.is_paid
      ? (state.settings?.paid_daily_token_limit ?? 7000000)
      : (state.settings?.free_daily_token_limit ?? 2000000);
    const tokensUsed = Number(u.daily_tokens_used || 0);
    const totalTokens = Number(u.total_tokens_used || tokensUsed);
    const fmtTok = (n) => (n >= 1_000_000 ? `${(n / 1_000_000).toFixed(2)}M` : n >= 1_000 ? `${(n / 1_000).toFixed(1)}k` : String(n));
    const tokenInfo = tokenCap > 0 ? `${fmtTok(tokensUsed)} / ${fmtTok(tokenCap)} tok` : `${fmtTok(tokensUsed)} tok`;
    const totalTokenInfo = `${fmtTok(totalTokens)} lifetime`;

    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td>
        <div class="user-email">${escapeHtml(u.email)}</div>
        <div class="user-id">${escapeHtml(u.id)}</div>
      </td>
      <td>
        <div>${tierBadge}</div>
        <div class="hint">${escapeHtml(passInfo)}</div>
        <div class="row-actions" style="margin-top:4px;">
          <button type="button" class="btn btn-sm btn-ghost" data-pass-days="15">+15d</button>
          <button type="button" class="btn btn-sm btn-ghost" data-pass-days="30">+30d</button>
          ${u.is_paid ? `<button type="button" class="btn btn-sm btn-danger-ghost" data-pass-days="0">Revoke</button>` : ""}
        </div>
      </td>
      <td>
        <div class="credits-cell" style="margin-bottom:4px;">
          <span class="hint" style="width:42px;">Daily</span>
          <input type="number" min="0" step="1" class="input mono credits-input" data-balance value="${dailyVal}" />
        </div>
        <div class="credits-cell">
          <span class="hint" style="width:42px;">Wallet</span>
          <input type="number" min="0" step="1" class="input mono credits-input" data-wallet value="${walletVal}" />
        </div>
        <div class="hint mono" style="margin-top:4px;">Today: ${escapeHtml(tokenInfo)} · ${escapeHtml(totalTokenInfo)}</div>
      </td>
      <td>${activeModelInfo}</td>
      <td class="col-actions">
        <div class="row-actions">
          <button type="button" class="btn btn-sm btn-primary" data-save-user>Save</button>
          <button type="button" class="btn btn-sm ${u.banned ? "btn-secondary" : "btn-danger-ghost"}" data-ban>
            ${u.banned ? "Unban" : "Ban"}
          </button>
          <button type="button" class="btn btn-sm btn-danger-ghost" data-delete-user title="Permanently delete user">Delete</button>
        </div>
      </td>
    `;

    const balInput = tr.querySelector("[data-balance]");
    const walletInput = tr.querySelector("[data-wallet]");
    const saveBtn = tr.querySelector("[data-save-user]");
    const banBtn = tr.querySelector("[data-ban]");
    const delBtn = tr.querySelector("[data-delete-user]");

    tr.querySelectorAll("[data-pass-days]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const days = Number(btn.dataset.passDays);
        btn.disabled = true;
        try {
          await api("/api/users", {
            method: "POST",
            body: JSON.stringify({ action: "set_pass", user_id: u.id, days }),
          });
          showToast(days > 0 ? `Granted +${days}d pass to ${u.email}` : `Revoked pass for ${u.email}`);
          await loadAll();
        } catch (err) {
          showToast(err.message, true);
          btn.disabled = false;
        }
      });
    });

    saveBtn.addEventListener("click", async () => {
      saveBtn.disabled = true;
      try {
        await api("/api/users", {
          method: "POST",
          body: JSON.stringify({
            action: "update_credits",
            user_id: u.id,
            balance: Number(balInput.value) || 0,
            wallet_credit: Number(walletInput.value) || 0,
          }),
        });
        u.balance = Number(balInput.value) || 0;
        u.wallet_credit = Number(walletInput.value) || 0;
        showToast(`Updated credits for ${u.email}`);
      } catch (err) {
        showToast(err.message, true);
      } finally {
        saveBtn.disabled = false;
      }
    });

    banBtn.addEventListener("click", async () => {
      const nextBanned = !u.banned;
      banBtn.disabled = true;
      try {
        await api("/api/users", {
          method: "POST",
          body: JSON.stringify({
            action: "set_ban",
            user_id: u.id,
            banned: nextBanned,
          }),
        });
        u.banned = nextBanned;
        renderUsers();
        showToast(nextBanned ? `Banned ${u.email}` : `Unbanned ${u.email}`);
      } catch (err) {
        showToast(err.message, true);
      } finally {
        banBtn.disabled = false;
      }
    });

    delBtn.addEventListener("click", async () => {
      if (!confirm(`Permanently delete ${u.email} (${u.id})? This cannot be undone.`)) return;
      delBtn.disabled = true;
      try {
        await api("/api/users", {
          method: "POST",
          body: JSON.stringify({ action: "delete_user", user_id: u.id }),
        });
        state.users = state.users.filter((x) => x.id !== u.id);
        renderUsers();
        showToast(`Deleted ${u.email}`);
      } catch (err) {
        showToast(err.message, true);
        delBtn.disabled = false;
      }
    });

    tbody.appendChild(tr);
  }
}

function renderDevices(devices) {
  const tbody = $("devices-tbody");
  if (!devices || devices.length === 0) {
    tbody.innerHTML = `<tr><td colspan="6" class="empty-cell">No device login sessions recorded.</td></tr>`;
    return;
  }
  tbody.innerHTML = "";
  for (const d of devices) {
    const expired = d.expires_at && Date.parse(d.expires_at) <= Date.now();
    const effectiveStatus = d.status === "pending" && expired ? "expired" : d.status;
    const badgeClass =
      effectiveStatus === "approved" || effectiveStatus === "consumed"
        ? "badge-active"
        : effectiveStatus === "denied"
          ? "badge-banned"
          : "badge-zero";

    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td class="mono">${escapeHtml(d.user_code)}</td>
      <td><span class="badge ${badgeClass}">${escapeHtml(effectiveStatus)}</span></td>
      <td>${escapeHtml(d.user_email || "—")}</td>
      <td class="mono">${Number(d.poll_count) || 0}</td>
      <td class="mono">${d.expires_at ? new Date(d.expires_at).toLocaleString() : "—"}</td>
      <td class="col-actions">
        ${
          effectiveStatus === "pending"
            ? `<button type="button" class="btn btn-sm btn-danger-ghost" data-deny>Deny</button>`
            : `<span class="hint">—</span>`
        }
      </td>
    `;
    const denyBtn = tr.querySelector("[data-deny]");
    if (denyBtn) {
      denyBtn.addEventListener("click", async () => {
        denyBtn.disabled = true;
        try {
          await api("/api/devices", {
            method: "POST",
            body: JSON.stringify({ action: "deny", user_code: d.user_code }),
          });
          showToast(`Denied code ${d.user_code}`);
          loadAll();
        } catch (err) {
          showToast(err.message, true);
          denyBtn.disabled = false;
        }
      });
    }
    tbody.appendChild(tr);
  }
}

function escapeHtml(str) {
  return String(str ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function escapeAttr(str) {
  return escapeHtml(str).replace(/'/g, "&#39;");
}

async function loadAll() {
  setGlobalError(null);
  try {
    const [settings, usersRes, devicesRes, paymentsRes] = await Promise.all([
      api("/api/settings"),
      api("/api/users"),
      api("/api/devices").catch(() => ({ devices: [] })),
      api("/api/payments").catch(() => ({ requests: [], pending_count: 0 })),
    ]);
    renderSettings(settings);
    state.users = usersRes.users || [];
    renderUsers();
    renderDevices(devicesRes.devices || []);
    renderPayments(paymentsRes.requests || [], paymentsRes.pending_count || 0);
  } catch (err) {
    setGlobalError(
      `Failed to load Supabase data: ${err.message}. Make sure the latest Supabase migrations are applied and SUPABASE_SERVICE_ROLE_KEY is set in Vercel.`,
    );
  }
}

// Event bindings
$("ai-enabled-toggle").addEventListener("change", async (e) => {
  const ai_enabled = e.target.checked;
  try {
    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify({ ai_enabled }),
    });
    renderSettings(updated);
    showToast(ai_enabled ? "AI is now ONLINE" : "AI is now PAUSED");
  } catch (err) {
    e.target.checked = !ai_enabled;
    showToast(err.message, true);
  }
});

$("save-provider-btn").addEventListener("click", async () => {
  const btn = $("save-provider-btn");
  btn.disabled = true;
  try {
    const payload = {
      llm_base_url: $("llm-base-url").value.trim(),
      llm_default_model: $("llm-default-model").value.trim(),
      free_daily_credits: Number($("free-daily-credits").value),
      paid_daily_credits: Number($("paid-daily-credits").value),
      free_daily_token_limit: Number($("free-daily-tokens").value),
      paid_daily_token_limit: Number($("paid-daily-tokens").value),
      free_max_steps: Number($("free-max-steps").value),
      paid_max_steps: Number($("paid-max-steps").value),
      pass_duration_minutes: Number($("pass-duration-minutes").value),
      credit_cost: Number($("credit-cost").value),
      free_rpm: Number($("free-rpm").value),
      paid_rpm: Number($("paid-rpm").value),
      free_burst_10s: Number($("free-burst-10s").value),
      paid_burst_10s: Number($("paid-burst-10s").value),
      free_tpm: Number($("free-tpm").value),
      paid_tpm: Number($("paid-tpm").value),
      free_max_concurrency: Number($("free-max-concurrency").value),
      paid_max_concurrency: Number($("paid-max-concurrency").value),
      free_max_input_tokens: Number($("free-max-input-tokens").value),
      paid_max_input_tokens: Number($("paid-max-input-tokens").value),
      free_max_output_tokens: Number($("free-max-output-tokens").value),
      paid_max_output_tokens: Number($("paid-max-output-tokens").value),
      ip_chat_rpm: Number($("ip-chat-rpm").value),
      global_chat_rpm: Number($("global-chat-rpm").value),
      global_daily_token_limit: Number($("global-daily-token-limit").value),
      payment_link: $("payment-link").value.trim(),
      topup_paused: $("topup-paused").checked,
    };
    const typedKey = $("llm-api-key").value.trim();
    if (typedKey) payload.llm_api_key = typedKey;

    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify(payload),
    });
    renderSettings(updated);
    showToast("Provider & billing settings saved");
  } catch (err) {
    showToast(err.message, true);
  } finally {
    btn.disabled = false;
  }
});

$("clear-key-btn").addEventListener("click", async () => {
  if (!confirm("Clear the stored LLM API key? Chat will fall back to Supabase env secret (or stop if unset).")) {
    return;
  }
  try {
    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify({ clear_api_key: true }),
    });
    renderSettings(updated);
    showToast("Stored API key cleared");
  } catch (err) {
    showToast(err.message, true);
  }
});

$("toggle-key-vis").addEventListener("click", () => {
  const inp = $("llm-api-key");
  const isPass = inp.type === "password";
  inp.type = isPass ? "text" : "password";
  $("toggle-key-vis").textContent = isPass ? "Hide" : "Show";
});

$("test-llm-btn").addEventListener("click", async () => {
  const btn = $("test-llm-btn");
  const box = $("test-result");
  btn.disabled = true;
  box.className = "inline-status";
  box.textContent = "Testing provider connection…";
  box.classList.remove("hidden");
  try {
    const res = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify({
        action: "test_llm",
        llm_base_url: $("llm-base-url").value.trim(),
        llm_api_key: $("llm-api-key").value.trim() || undefined,
      }),
    });
    if (res.ok) {
      box.className = "inline-status ok";
      box.textContent = `Connected (HTTP ${res.status}, ${res.latency_ms} ms).`;
    } else {
      box.className = "inline-status err";
      box.textContent = `Connection failed${res.status ? ` (HTTP ${res.status})` : ""}: ${res.error}`;
    }
  } catch (err) {
    box.className = "inline-status err";
    box.textContent = `Test error: ${err.message}`;
  } finally {
    btn.disabled = false;
  }
});

document.querySelectorAll(".preset-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    $("llm-base-url").value = btn.dataset.url;
    $("llm-default-model").value = btn.dataset.model;
    showToast(`Applied ${btn.textContent} preset (click Save Settings)`);
  });
});

$("add-model-btn").addEventListener("click", () => {
  state.models.push({ id: "", label: "", context_window: 128000, credit_cost: 3, paid_only: false });
  renderModels();
});

$("save-models-btn").addEventListener("click", async () => {
  const btn = $("save-models-btn");
  btn.disabled = true;
  try {
    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify({
        models: state.models.map(({ has_api_key, api_key_masked, ...m }) => ({
          ...m,
          label: (m.display_name || "").trim() || m.label || m.id,
        })),
      }),
    });
    renderSettings(updated);
    showToast("Model catalog saved");
  } catch (err) {
    showToast(err.message, true);
  } finally {
    btn.disabled = false;
  }
});

$("user-search").addEventListener("input", renderUsers);

$("toggle-create-user-btn").addEventListener("click", () => {
  $("create-user-form").classList.toggle("hidden");
});

$("create-user-submit").addEventListener("click", async () => {
  const btn = $("create-user-submit");
  btn.disabled = true;
  try {
    await api("/api/users", {
      method: "POST",
      body: JSON.stringify({
        action: "create_user",
        email: $("new-user-email").value.trim(),
        password: $("new-user-password").value,
        plan: $("new-user-plan").value,
        balance: Number($("new-user-credits").value) || 6,
      }),
    });
    $("new-user-email").value = "";
    $("new-user-password").value = "";
    $("create-user-form").classList.add("hidden");
    showToast("User created");
    await loadAll();
  } catch (err) {
    showToast(err.message, true);
  } finally {
    btn.disabled = false;
  }
});

$("purge-devices-btn").addEventListener("click", async () => {
  try {
    await api("/api/devices", {
      method: "POST",
      body: JSON.stringify({ action: "purge_expired" }),
    });
    showToast("Expired device codes cleaned");
    await loadAll();
  } catch (err) {
    showToast(err.message, true);
  }
});

$("refresh-btn").addEventListener("click", loadAll);

$("lock-btn").addEventListener("click", async () => {
  try {
    await api("/api/auth", {
      method: "POST",
      body: JSON.stringify({ action: "logout" }),
    });
  } catch {
    // ignore logout network error
  }
  showAuthGate("");
});

$("auth-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("auth-submit-btn");
  const input = $("auth-passphrase");
  const passphrase = input.value.trim();
  if (!passphrase) return;
  btn.disabled = true;
  $("auth-error").classList.add("hidden");
  try {
    await api("/api/auth", {
      method: "POST",
      body: JSON.stringify({ passphrase }),
    });
    input.value = "";
    showConsole();
    await loadAll();
  } catch (err) {
    showAuthGate(err.message || "Invalid passphrase.");
  } finally {
    btn.disabled = false;
  }
});

async function bootAuth() {
  try {
    const status = await api("/api/auth");
    if (!status.configured) {
      showAuthGate("ADMIN_PASSPHRASE is not configured in Vercel Project Settings.");
      return;
    }
    if (status.authenticated) {
      showConsole();
      await loadAll();
    } else {
      showAuthGate("");
    }
  } catch (err) {
    showAuthGate(err.message || "Unable to verify session.");
  }
}

bootAuth();
