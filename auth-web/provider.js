import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";
const PROVIDERS_ENDPOINT = `${SUPABASE_URL}/functions/v1/providers`;

const PRESETS = {
  openrouter: { label: "OpenRouter", base_url: "https://openrouter.ai/api/v1", exampleModel: "anthropic/claude-sonnet-4" },
  openai: { label: "OpenAI", base_url: "https://api.openai.com/v1", exampleModel: "gpt-4o" },
  deepseek: { label: "DeepSeek", base_url: "https://api.deepseek.com/v1", exampleModel: "deepseek-chat" },
  groq: { label: "Groq", base_url: "https://api.groq.com/openai/v1", exampleModel: "llama-3.3-70b-versatile" },
  custom: { label: "Custom Provider", base_url: "", exampleModel: "model-id" },
};

const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});

const $ = (id) => document.getElementById(id);

let currentSession = null;
let savedProviders = [];

function escapeHtml(s) {
  return String(s ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function showBanner(message, kind = "error") {
  const el = $("status-banner");
  if (!message) {
    el.className = "alert hidden";
    el.textContent = "";
    return;
  }
  el.textContent = message;
  el.className = `alert ${kind === "success" ? "alert-success" : "alert-error"}`;
}

function addModelRow(model = { id: "", display_name: "", context_window: 128000 }) {
  const container = $("models-editor");
  const row = document.createElement("div");
  row.className = "model-row";
  const preset = PRESETS[$("preset-select").value] || PRESETS.custom;

  row.innerHTML = `
    <input type="text" class="mono model-id-input" placeholder="${escapeHtml(preset.exampleModel)}" value="${escapeHtml(model.id || "")}" required />
    <input type="text" class="model-name-input" placeholder="Display name (optional)" value="${escapeHtml(model.display_name || "")}" />
    <input type="number" class="mono model-ctx-input" min="1000" max="10000000" step="1000" placeholder="128000" value="${escapeHtml(model.context_window || 128000)}" title="Context window (tokens)" />
    <button type="button" class="btn btn-sm btn-secondary btn-remove-model" title="Remove model">✕</button>
  `;

  row.querySelector(".btn-remove-model").addEventListener("click", () => {
    if (container.children.length > 1) {
      row.remove();
    } else {
      row.querySelector(".model-id-input").value = "";
      row.querySelector(".model-name-input").value = "";
    }
  });

  container.appendChild(row);
}

function collectModelsFromEditor() {
  const rows = [...$("models-editor").querySelectorAll(".model-row")];
  const models = [];
  for (const row of rows) {
    const id = row.querySelector(".model-id-input").value.trim();
    if (!id) continue;
    const displayName = row.querySelector(".model-name-input").value.trim();
    const ctxVal = Number(row.querySelector(".model-ctx-input").value);
    models.push({
      id,
      ...(displayName ? { display_name: displayName } : {}),
      context_window: Number.isFinite(ctxVal) && ctxVal >= 1000 ? Math.floor(ctxVal) : 128000,
    });
  }
  return models;
}

function resetForm() {
  $("edit-provider-id").value = "";
  $("form-heading").textContent = "Add Provider";
  $("btn-cancel-edit").classList.add("hidden");
  $("btn-save-provider").textContent = "Save Provider";
  $("preset-select").value = "openrouter";
  $("provider-name").value = PRESETS.openrouter.label;
  $("base-url").value = PRESETS.openrouter.base_url;
  $("api-key").value = "";
  $("api-key").required = true;
  $("api-key-hint-text").textContent = "Encrypted with AES-256-GCM at rest. Never shown again after saving.";
  $("models-editor").innerHTML = "";
  addModelRow();
}

function startEditProvider(provider) {
  $("edit-provider-id").value = provider.id;
  $("form-heading").textContent = `Edit ${provider.name}`;
  $("btn-cancel-edit").classList.remove("hidden");
  $("btn-save-provider").textContent = "Update Provider";
  $("preset-select").value = provider.preset in PRESETS ? provider.preset : "custom";
  $("provider-name").value = provider.name || "";
  $("base-url").value = provider.base_url || "";
  $("api-key").value = "";
  $("api-key").required = false;
  $("api-key-hint-text").textContent = `Current key: ${provider.api_key_hint}. Leave blank to keep existing key, or paste a new key to rotate it.`;
  $("models-editor").innerHTML = "";
  const models = Array.isArray(provider.models) && provider.models.length > 0 ? provider.models : [{ id: "" }];
  for (const m of models) addModelRow(m);
  window.scrollTo({ top: 0, behavior: "smooth" });
}

function renderProviders() {
  const listEl = $("providers-list");
  $("provider-count").textContent = String(savedProviders.length);

  if (savedProviders.length === 0) {
    listEl.innerHTML = `<div class="empty-state">No BYOK providers saved yet. Add one on the left and its models will appear in <code>/model</code> inside Openmi CLI.</div>`;
    return;
  }

  listEl.innerHTML = savedProviders
    .map((p) => {
      const pills = (Array.isArray(p.models) ? p.models : [])
        .map((m) => `<span class="model-pill" title="${escapeHtml(m.id)}">${escapeHtml(m.display_name || m.id)}</span>`)
        .join("");
      return `
        <article class="provider-item" data-id="${escapeHtml(p.id)}">
          <div class="provider-item-head">
            <div class="provider-title">
              <span>${escapeHtml(p.name)}</span>
              <span class="badge badge-free">${escapeHtml(p.preset || "custom")}</span>
            </div>
            <div class="provider-actions">
              <button type="button" class="btn btn-sm btn-secondary btn-edit-provider">Edit</button>
              <button type="button" class="btn btn-sm btn-danger btn-delete-provider">Delete</button>
            </div>
          </div>
          <div class="provider-meta mono">${escapeHtml(p.base_url)} · Key: ${escapeHtml(p.api_key_hint)}</div>
          <div class="model-pills">${pills}</div>
        </article>
      `;
    })
    .join("");

  for (const card of listEl.querySelectorAll(".provider-item")) {
    const id = card.getAttribute("data-id");
    const provider = savedProviders.find((p) => p.id === id);
    if (!provider) continue;

    card.querySelector(".btn-edit-provider").addEventListener("click", () => {
      showBanner("");
      startEditProvider(provider);
    });

    card.querySelector(".btn-delete-provider").addEventListener("click", async () => {
      if (!confirm(`Delete provider "${provider.name}" and remove its models from /model?`)) return;
      await deleteProvider(provider.id);
    });
  }
}

async function apiFetch(url, options = {}) {
  if (!currentSession?.access_token) {
    throw new Error("Please sign in first.");
  }
  const resp = await fetch(url, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${currentSession.access_token}`,
      ...(options.headers || {}),
    },
  });
  const data = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new Error(data.message || data.error || `Request failed (HTTP ${resp.status})`);
  }
  return data;
}

async function loadProviders() {
  showBanner("");
  try {
    const data = await apiFetch(PROVIDERS_ENDPOINT, { method: "GET" });
    savedProviders = Array.isArray(data.providers) ? data.providers : [];
    renderProviders();
  } catch (err) {
    showBanner(err instanceof Error ? err.message : String(err), "error");
  }
}

async function deleteProvider(id) {
  showBanner("");
  try {
    await apiFetch(`${PROVIDERS_ENDPOINT}?id=${encodeURIComponent(id)}`, { method: "DELETE" });
    if ($("edit-provider-id").value === id) resetForm();
    showBanner("Provider removed. Run /model in Openmi CLI to refresh.", "success");
    await loadProviders();
  } catch (err) {
    showBanner(err instanceof Error ? err.message : String(err), "error");
  }
}

async function handleFormSubmit(ev) {
  ev.preventDefault();
  showBanner("");

  const editId = $("edit-provider-id").value.trim();
  const preset = $("preset-select").value;
  const name = $("provider-name").value.trim();
  const baseUrl = $("base-url").value.trim();
  const apiKey = $("api-key").value.trim();
  const models = collectModelsFromEditor();

  if (models.length === 0) {
    showBanner("Add at least one model ID.", "error");
    return;
  }
  if (!editId && !apiKey) {
    showBanner("API key is required.", "error");
    return;
  }

  const saveBtn = $("btn-save-provider");
  saveBtn.disabled = true;
  try {
    if (editId) {
      await apiFetch(PROVIDERS_ENDPOINT, {
        method: "PATCH",
        body: JSON.stringify({
          id: editId,
          preset,
          name,
          base_url: baseUrl,
          ...(apiKey ? { api_key: apiKey } : {}),
          models,
        }),
      });
      showBanner("Provider updated! Run /model in Openmi CLI to use your models.", "success");
    } else {
      await apiFetch(PROVIDERS_ENDPOINT, {
        method: "POST",
        body: JSON.stringify({
          preset,
          name,
          base_url: baseUrl,
          api_key: apiKey,
          models,
        }),
      });
      showBanner("Provider saved! Run /model in Openmi CLI to select your custom models.", "success");
    }
    resetForm();
    await loadProviders();
  } catch (err) {
    showBanner(err instanceof Error ? err.message : String(err), "error");
  } finally {
    saveBtn.disabled = false;
  }
}

function applySession(session) {
  currentSession = session;
  if (session?.user) {
    $("sidebar-account").classList.remove("hidden");
    $("sidebar-signin-btn").classList.add("hidden");
    $("sidebar-email").textContent = session.user.email || session.user.id;
    $("signed-out-section").classList.add("hidden");
    $("byok-section").classList.remove("hidden");
    loadProviders();
  } else {
    $("sidebar-account").classList.add("hidden");
    $("sidebar-signin-btn").classList.remove("hidden");
    $("signed-out-section").classList.remove("hidden");
    $("byok-section").classList.add("hidden");
  }
}

$("preset-select").addEventListener("change", () => {
  const key = $("preset-select").value;
  const preset = PRESETS[key] || PRESETS.custom;
  if (!$("edit-provider-id").value) {
    $("provider-name").value = preset.label;
    if (preset.base_url) $("base-url").value = preset.base_url;
  } else if (preset.base_url) {
    $("base-url").value = preset.base_url;
  }
});

$("btn-add-model-row").addEventListener("click", () => addModelRow());
$("btn-cancel-edit").addEventListener("click", () => {
  showBanner("");
  resetForm();
});
$("provider-form").addEventListener("submit", handleFormSubmit);
$("btn-refresh").addEventListener("click", () => {
  if (currentSession) loadProviders();
});
$("btn-signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  applySession(null);
});

resetForm();
supabase.auth.getSession().then(({ data }) => applySession(data.session));
supabase.auth.onAuthStateChange((_event, session) => applySession(session));
