const $ = (id) => document.getElementById(id);

let state = {
  settings: null,
  models: [],
  users: [],
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

async function api(path, options = {}) {
  const resp = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(options.headers || {}) },
    ...options,
  });
  const data = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new Error(data.error || `HTTP ${resp.status}`);
  }
  return data;
}

function renderSettings(s) {
  state.settings = s;
  state.models = (s.models || []).map((m) => ({ ...m }));

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
  $("metric-cost").textContent = s.credit_cost === 0 ? "Free (0)" : `${s.credit_cost} / step`;

  $("llm-base-url").value = s.llm_base_url || "";
  $("llm-default-model").value = s.llm_default_model || "";
  $("credit-cost").value = s.credit_cost ?? 1;
  $("starter-credits").value = s.starter_credits ?? 100;
  $("llm-api-key").value = "";
  $("current-key-hint").textContent = s.has_api_key
    ? `Stored: ${s.llm_api_key_masked}`
    : "No DB key stored (falls back to Supabase secret)";

  renderModels();
}

function renderModels() {
  const tbody = $("models-tbody");
  tbody.innerHTML = "";
  if (state.models.length === 0) {
    tbody.innerHTML = `<tr><td colspan="4" class="empty-cell">No models configured. Click "+ Add Model".</td></tr>`;
    return;
  }

  state.models.forEach((m, idx) => {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td><input type="text" class="input mono" data-field="id" value="${escapeAttr(m.id)}" placeholder="provider/model-id" /></td>
      <td><input type="text" class="input" data-field="label" value="${escapeAttr(m.label)}" placeholder="Display Name" /></td>
      <td><input type="number" min="1000" step="1000" class="input mono" data-field="context_window" value="${Number(m.context_window) || 128000}" /></td>
      <td class="col-actions"><button type="button" class="btn btn-sm btn-danger-ghost" data-remove="${idx}">Remove</button></td>
    `;

    tr.querySelectorAll("input").forEach((inp) => {
      inp.addEventListener("input", () => {
        const field = inp.dataset.field;
        state.models[idx][field] =
          field === "context_window" ? Number(inp.value) || 128000 : inp.value;
      });
    });

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
    const balanceVal = u.balance === null ? (state.settings?.starter_credits ?? 100) : u.balance;
    const statusBadge = u.banned
      ? `<span class="badge badge-banned">Banned</span>`
      : balanceVal === 0
        ? `<span class="badge badge-zero">0 Credits</span>`
        : `<span class="badge badge-active">Active</span>`;

    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td>
        <div class="user-email">${escapeHtml(u.email)}</div>
        <div class="user-id">${escapeHtml(u.id)}</div>
      </td>
      <td>
        <select class="select plan-select" data-plan>
          ${["free", "pro", "team", "enterprise"]
            .concat(["free", "pro", "team", "enterprise"].includes(u.plan) ? [] : [u.plan])
            .map(
              (p) => `<option value="${escapeAttr(p)}" ${u.plan === p ? "selected" : ""}>${escapeHtml(p)}</option>`,
            )
            .join("")}
        </select>
      </td>
      <td>
        <div class="credits-cell">
          <input type="number" min="0" step="1" class="input mono credits-input" data-balance value="${balanceVal}" />
          <button type="button" class="btn btn-sm btn-ghost" data-add="100" title="Add 100 credits">+100</button>
          <button type="button" class="btn btn-sm btn-ghost" data-zero title="Set credits to 0">0</button>
        </div>
      </td>
      <td>${statusBadge}</td>
      <td class="col-actions">
        <div class="row-actions">
          <button type="button" class="btn btn-sm btn-primary" data-save-user>Save</button>
          <button type="button" class="btn btn-sm ${u.banned ? "btn-secondary" : "btn-danger-ghost"}" data-ban>
            ${u.banned ? "Unban" : "Ban"}
          </button>
        </div>
      </td>
    `;

    const balInput = tr.querySelector("[data-balance]");
    const planSelect = tr.querySelector("[data-plan]");
    const saveBtn = tr.querySelector("[data-save-user]");
    const banBtn = tr.querySelector("[data-ban]");

    tr.querySelector("[data-add]").addEventListener("click", () => {
      balInput.value = String((Number(balInput.value) || 0) + 100);
    });
    tr.querySelector("[data-zero]").addEventListener("click", () => {
      balInput.value = "0";
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
            plan: planSelect.value,
          }),
        });
        u.balance = Number(balInput.value) || 0;
        u.plan = planSelect.value;
        renderUsers();
        showToast(`Updated ${u.email}`);
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
    const [settings, usersRes] = await Promise.all([api("/api/settings"), api("/api/users")]);
    renderSettings(settings);
    state.users = usersRes.users || [];
    renderUsers();
  } catch (err) {
    setGlobalError(
      `Failed to load Supabase data: ${err.message}. Make sure migration 20261126000003_create_app_settings.sql is applied and SUPABASE_SERVICE_ROLE_KEY is set in Vercel.`,
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
      credit_cost: Number($("credit-cost").value),
      starter_credits: Number($("starter-credits").value),
    };
    const typedKey = $("llm-api-key").value.trim();
    if (typedKey) payload.llm_api_key = typedKey;

    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify(payload),
    });
    renderSettings(updated);
    showToast("Provider settings saved");
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
    showToast(`Applied ${btn.textContent} preset — click Save Settings`);
  });
});

$("add-model-btn").addEventListener("click", () => {
  state.models.push({ id: "", label: "", context_window: 128000 });
  renderModels();
});

$("save-models-btn").addEventListener("click", async () => {
  const btn = $("save-models-btn");
  btn.disabled = true;
  try {
    const updated = await api("/api/settings", {
      method: "POST",
      body: JSON.stringify({ models: state.models }),
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
$("refresh-btn").addEventListener("click", loadAll);

loadAll();
