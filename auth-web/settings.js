import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";

const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});

const $ = (id) => document.getElementById(id);

let currentSession = null;
let currentIdentities = [];

function fmtDate(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
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

function friendlyAuthError(rawMsg, errorCode = "") {
  const msg = String(rawMsg || "").trim();
  const lower = `${errorCode} ${msg}`.toLowerCase();
  if (lower.includes("identity_already_exists") || lower.includes("already linked") || lower.includes("already exists")) {
    return "That account is already linked to another Openmi user. Sign out and sign in with that provider if you meant to use the other account.";
  }
  if (lower.includes("manual linking is disabled")) {
    return "Account binding is not enabled on the auth server yet (enable Manual Linking in Supabase Authentication settings).";
  }
  if (lower.includes("single_identity_not_deletable") || lower.includes("at least 2")) {
    return "You must keep at least one sign-in method connected to your account.";
  }
  return msg || "Could not complete account action.";
}

function consumeRedirectFeedback() {
  const url = new URL(window.location.href);
  const searchParams = url.searchParams;
  const hashParams = new URLSearchParams(url.hash.startsWith("#") ? url.hash.slice(1) : "");

  const errCode = searchParams.get("error_code") || hashParams.get("error_code") || "";
  const errDesc =
    searchParams.get("error_description") ||
    hashParams.get("error_description") ||
    searchParams.get("error") ||
    hashParams.get("error") ||
    "";
  const linked = searchParams.get("linked") || "";

  let handled = false;
  if (errDesc || errCode) {
    showBanner(friendlyAuthError(errDesc, errCode), "error");
    handled = true;
  } else if (linked) {
    const label = linked === "google" ? "Google" : linked === "github" ? "GitHub" : linked;
    showBanner(`${label} account bound to your Openmi account!`, "success");
    handled = true;
  }

  if (handled) {
    const cleanUrl = `${url.origin}${url.pathname}`;
    window.history.replaceState({}, document.title, cleanUrl);
  }
}

function getIdentitySummary(provider, identity) {
  const d = identity?.identity_data || {};
  const parts = [];
  if (provider === "github") {
    const handle = d.user_name || d.preferred_username;
    if (handle) parts.push(`@${handle}`);
    if (d.email) parts.push(d.email);
  } else if (provider === "google") {
    if (d.email) parts.push(d.email);
    else if (d.full_name || d.name) parts.push(d.full_name || d.name);
  } else if (provider === "email") {
    if (d.email || currentSession?.user?.email) {
      parts.push(d.email || currentSession.user.email);
    }
  }
  if (identity?.created_at) {
    parts.push(`Linked ${fmtDate(identity.created_at)}`);
  }
  return parts.join(" · ");
}

function createActionButton(text, className, onClick, { disabled = false, title = "" } = {}) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = className;
  btn.textContent = text;
  btn.disabled = disabled;
  if (title) btn.title = title;
  btn.addEventListener("click", onClick);
  return btn;
}

function renderOAuthRow(provider, label, identity, totalIdentities) {
  const badge = $(`badge-${provider}`);
  const detail = $(`detail-${provider}`);
  const actions = $(`actions-${provider}`);
  actions.innerHTML = "";

  if (identity) {
    badge.textContent = "Connected";
    badge.className = "badge badge-paid";
    detail.textContent = getIdentitySummary(provider, identity) || `Connected via ${label}`;

    const canUnbind = totalIdentities > 1;
    const unbindBtn = createActionButton(
      "Unbind",
      "btn btn-sm btn-danger",
      () => handleUnbind(provider, label, identity),
      {
        disabled: !canUnbind,
        title: canUnbind
          ? `Unbind ${label} from this account`
          : "Bind another sign-in method before unbinding your only login",
      },
    );
    actions.appendChild(unbindBtn);
  } else {
    badge.textContent = "Not connected";
    badge.className = "badge badge-free";
    detail.textContent =
      provider === "google"
        ? "Bind your Google account for one-click sign-in."
        : "Bind your GitHub account for developer sign-in.";

    const bindBtn = createActionButton(
      `Bind ${label}`,
      "btn btn-sm btn-primary",
      () => handleBindOAuth(provider, label, bindBtn),
    );
    actions.appendChild(bindBtn);
  }
}

function renderEmailRow(emailIdentity, totalIdentities) {
  const badge = $("badge-email");
  const detail = $("detail-email");
  const actions = $("actions-email");
  actions.innerHTML = "";

  const userEmail = currentSession?.user?.email || "";

  if (emailIdentity) {
    badge.textContent = "Connected";
    badge.className = "badge badge-paid";
    detail.textContent = getIdentitySummary("email", emailIdentity) || userEmail || "Email & password enabled";

    actions.appendChild(
      createActionButton("Change Password", "btn btn-sm btn-secondary", () => openPasswordPanel(true)),
    );

    const canUnbind = totalIdentities > 1;
    actions.appendChild(
      createActionButton(
        "Unbind",
        "btn btn-sm btn-danger",
        () => handleUnbind("email", "Email & Password", emailIdentity),
        {
          disabled: !canUnbind,
          title: canUnbind
            ? "Remove Email & Password sign-in"
            : "Bind Google or GitHub before unbinding Email & Password",
        },
      ),
    );
  } else {
    badge.textContent = "No password set";
    badge.className = "badge badge-free";
    detail.textContent = userEmail
      ? `Set a password to also sign in with ${userEmail}.`
      : "Add an email and password to sign in without OAuth.";

    actions.appendChild(
      createActionButton("Set Password", "btn btn-sm btn-primary", () => openPasswordPanel(false)),
    );
  }
}

function renderIdentities() {
  const total = currentIdentities.length;
  const byProvider = new Map();
  for (const id of currentIdentities) {
    if (id?.provider && !byProvider.has(id.provider)) {
      byProvider.set(id.provider, id);
    }
  }

  $("sidebar-linked-count").textContent = `${total} linked`;
  $("linked-methods-badge").textContent = `${total} of 3 connected`;
  $("linked-methods-badge").className = total >= 2 ? "badge badge-paid" : "badge badge-free";

  renderOAuthRow("google", "Google", byProvider.get("google") || null, total);
  renderOAuthRow("github", "GitHub", byProvider.get("github") || null, total);
  renderEmailRow(byProvider.get("email") || null, total);
}

function openPasswordPanel(isChange) {
  const emailInput = $("pw-email-input");
  const userEmail = currentSession?.user?.email || "";
  emailInput.value = userEmail;
  emailInput.readOnly = Boolean(userEmail);
  $("pw-new-input").value = "";
  $("pw-confirm-input").value = "";
  $("btn-save-password").textContent = isChange ? "Update Password" : "Save Password";
  $("password-panel").classList.remove("hidden");
  $("pw-new-input").focus();
}

function closePasswordPanel() {
  $("password-panel").classList.add("hidden");
  $("pw-new-input").value = "";
  $("pw-confirm-input").value = "";
}

async function handleBindOAuth(provider, label, btn) {
  showBanner("");
  btn.disabled = true;
  const prevText = btn.textContent;
  btn.textContent = `Connecting ${label}…`;

  try {
    const redirectTo = `${window.location.origin}/settings?linked=${encodeURIComponent(provider)}`;
    const { error } = await supabase.auth.linkIdentity({
      provider,
      options: { redirectTo },
    });
    if (error) throw error;
    // Supabase redirects the browser to the OAuth provider automatically.
  } catch (err) {
    showBanner(friendlyAuthError(err?.message, err?.code), "error");
    btn.disabled = false;
    btn.textContent = prevText;
  }
}

async function handleUnbind(provider, label, identity) {
  showBanner("");
  const confirmed = window.confirm(
    `Unbind ${label} from your Openmi account?\n\nYou will no longer be able to sign in with ${label} unless you bind it again.`,
  );
  if (!confirmed) return;

  try {
    const { error } = await supabase.auth.unlinkIdentity(identity);
    if (error) throw error;
    showBanner(`${label} has been unbound from your account.`, "success");
    await loadIdentities();
  } catch (err) {
    showBanner(friendlyAuthError(err?.message, err?.code), "error");
  }
}

async function loadIdentities() {
  if (!currentSession?.user) return;
  try {
    const { data, error } = await supabase.auth.getUserIdentities();
    if (error) throw error;
    currentIdentities = Array.isArray(data?.identities) ? data.identities : [];
  } catch (err) {
    // Fallback to identities embedded in the session user object
    currentIdentities = Array.isArray(currentSession.user.identities)
      ? currentSession.user.identities
      : [];
    showBanner(`Could not refresh linked accounts: ${err.message}`, "error");
  }
  renderIdentities();
}

function applyAuth(session) {
  currentSession = session;
  const signedIn = Boolean(session?.user);

  $("sidebar-account").classList.toggle("hidden", !signedIn);
  $("sidebar-signin-btn").classList.toggle("hidden", signedIn);
  $("signed-out-section").classList.toggle("hidden", signedIn);
  $("settings-section").classList.toggle("hidden", !signedIn);

  if (signedIn) {
    const user = session.user;
    $("sidebar-email").textContent = user.email || user.id;
    $("meta-email").textContent = user.email || "No email set";
    $("meta-user-id").textContent = user.id;
    $("meta-created-at").textContent = fmtDate(user.created_at);
    loadIdentities();
  } else {
    currentIdentities = [];
    closePasswordPanel();
  }
}

// Event bindings
$("password-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  showBanner("");

  const email = $("pw-email-input").value.trim();
  const password = $("pw-new-input").value;
  const confirm = $("pw-confirm-input").value;

  if (password.length < 6) {
    showBanner("Password must be at least 6 characters.", "error");
    return;
  }
  if (password !== confirm) {
    showBanner("Passwords do not match.", "error");
    return;
  }

  const btn = $("btn-save-password");
  btn.disabled = true;
  const prevText = btn.textContent;
  btn.textContent = "Saving…";

  try {
    const hadEmail = Boolean(currentSession?.user?.email);
    const payload = { password };
    if (!hadEmail && email) {
      payload.email = email;
    }
    const { data, error } = await supabase.auth.updateUser(payload);
    if (error) throw error;

    closePasswordPanel();
    if (data?.user) {
      currentSession = { ...currentSession, user: data.user };
      $("meta-email").textContent = data.user.email || "No email set";
    }

    const needsConfirm = !hadEmail && Boolean(email);
    showBanner(
      needsConfirm
        ? "Password saved! Check your inbox to confirm your email address."
        : "Password saved! You can now sign in with Email & Password on this account.",
      "success",
    );
    await loadIdentities();
  } catch (err) {
    showBanner(friendlyAuthError(err?.message, err?.code), "error");
  } finally {
    btn.disabled = false;
    btn.textContent = prevText;
  }
});

$("btn-cancel-password").addEventListener("click", closePasswordPanel);

$("btn-copy-uid").addEventListener("click", async () => {
  const uid = currentSession?.user?.id || "";
  if (!uid) return;
  try {
    await navigator.clipboard.writeText(uid);
    const btn = $("btn-copy-uid");
    btn.textContent = "Copied";
    setTimeout(() => {
      btn.textContent = "Copy";
    }, 1500);
  } catch (_) {
    // ignore clipboard errors
  }
});

$("btn-refresh").addEventListener("click", async () => {
  const btn = $("btn-refresh");
  btn.disabled = true;
  btn.textContent = "Refreshing…";
  try {
    const { data } = await supabase.auth.getSession();
    applyAuth(data.session);
  } finally {
    btn.disabled = false;
    btn.textContent = "Refresh";
  }
});

$("btn-signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  window.location.reload();
});

// Boot: let Supabase process any OAuth callback params before cleaning the URL.
const { data } = await supabase.auth.getSession();
consumeRedirectFeedback();
applyAuth(data.session);

supabase.auth.onAuthStateChange((_event, session) => {
  applyAuth(session);
});
