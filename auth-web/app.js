import { createClient } from "https://esm.sh/@supabase/supabase-js@2";

const SUPABASE_URL = "https://umjlmqeghfrfpduhkits.supabase.co";
const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_5VfWWlMRio4X-dCfq8IZnw_oYPbh6db";
const APPROVE_ENDPOINT = `${SUPABASE_URL}/functions/v1/cli-auth/approve`;
const CODE_STORAGE_KEY = "openmi_pending_device_code";
const REF_STORAGE_KEY = "openmi_pending_ref_code";

const initialRef = (new URLSearchParams(window.location.search).get("ref") || "").trim().toUpperCase();
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

let isSignUpMode = false;
let currentSession = null;

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

let isCliFlow = false;

function initDeviceCode() {
  const params = new URLSearchParams(window.location.search);
  const fromUrl = (params.get("code") || "").trim().toUpperCase();
  const isVerifyPath = window.location.pathname.startsWith("/verify");
  if (fromUrl) {
    sessionStorage.setItem(CODE_STORAGE_KEY, fromUrl);
    $("device-code").value = fromUrl;
    isCliFlow = true;
  } else if (isVerifyPath) {
    const saved = (sessionStorage.getItem(CODE_STORAGE_KEY) || "").trim().toUpperCase();
    if (saved) {
      $("device-code").value = saved;
      isCliFlow = true;
    }
  } else {
    // Normal /login web sign-in: clear any stale CLI code so the user goes to Dashboard.
    sessionStorage.removeItem(CODE_STORAGE_KEY);
    $("device-code").value = "";
    isCliFlow = false;
  }

  const codeBox = $("code-box");
  if (codeBox) codeBox.classList.toggle("hidden", !isCliFlow);
  const titleEl = $("card-title");
  if (titleEl) {
    titleEl.textContent = isCliFlow ? "Authorize Openmi CLI" : "Sign in to Openmi";
  }
}

$("device-code").addEventListener("input", (e) => {
  const val = e.target.value.trim().toUpperCase();
  if (val) sessionStorage.setItem(CODE_STORAGE_KEY, val);
});

const NEXT_STORAGE_KEY = "openmi_post_login_next";

function getSafeNextPath() {
  const params = new URLSearchParams(window.location.search);
  const fromUrl = (params.get("next") || "").trim();
  if (fromUrl && fromUrl.startsWith("/") && !fromUrl.startsWith("//")) {
    sessionStorage.setItem(NEXT_STORAGE_KEY, fromUrl);
    return fromUrl;
  }
  const saved = (sessionStorage.getItem(NEXT_STORAGE_KEY) || "").trim();
  if (saved && saved.startsWith("/") && !saved.startsWith("//")) {
    return saved;
  }
  return "";
}

function renderView(session) {
  currentSession = session;
  $("done-view").classList.add("hidden");
  if (session?.user) {
    if (!isCliFlow) {
      const nextPath = getSafeNextPath() || "/dashboard";
      sessionStorage.removeItem(NEXT_STORAGE_KEY);
      window.location.replace(nextPath);
      return;
    }
    $("auth-view").classList.add("hidden");
    $("approve-view").classList.remove("hidden");
    $("signed-in-email").textContent = session.user.email || session.user.id;
    $("card-subtitle").textContent = "Confirm the code shown in your terminal to approve access.";
  } else {
    $("approve-view").classList.add("hidden");
    $("auth-view").classList.remove("hidden");
    $("card-subtitle").textContent = isCliFlow
      ? "Sign in to connect your terminal session."
      : "Sign in to access your Openmi dashboard and billing.";
  }
}

async function startOAuth(provider) {
  showStatus("");
  const code = isCliFlow ? $("device-code").value.trim().toUpperCase() : "";
  if (code) sessionStorage.setItem(CODE_STORAGE_KEY, code);

  const nextPath = getSafeNextPath();
  const redirectUrl = code
    ? new URL("/verify", window.location.origin)
    : new URL(nextPath || "/dashboard", window.location.origin);
  if (code) redirectUrl.searchParams.set("code", code);

  const { error } = await supabase.auth.signInWithOAuth({
    provider,
    options: {
      redirectTo: redirectUrl.toString(),
    },
  });
  if (error) {
    showStatus(`Could not start ${provider} sign-in: ${error.message}`, "error");
  }
}

$("btn-google").addEventListener("click", () => startOAuth("google"));
$("btn-github").addEventListener("click", () => startOAuth("github"));

$("toggle-mode").addEventListener("click", () => {
  isSignUpMode = !isSignUpMode;
  $("email-submit").textContent = isSignUpMode ? "Create Account" : "Sign in with Email";
  $("mode-prompt").textContent = isSignUpMode
    ? "Already have an account?"
    : "Don't have an account?";
  $("toggle-mode").textContent = isSignUpMode ? "Sign in" : "Create account";
  $("password-input").autocomplete = isSignUpMode ? "new-password" : "current-password";
  showStatus("");
});

$("email-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const btn = $("email-submit");
  const email = $("email-input").value.trim();
  const password = $("password-input").value;
  btn.disabled = true;
  showStatus("");

  try {
    if (isSignUpMode) {
      const { data, error } = await supabase.auth.signUp({ email, password });
      if (error) throw error;
      if (data.session) {
        renderView(data.session);
      } else {
        showStatus(
          "Account created! Check your email to confirm your address, then sign in.",
          "ok",
        );
      }
    } else {
      const { data, error } = await supabase.auth.signInWithPassword({ email, password });
      if (error) throw error;
      renderView(data.session);
    }
  } catch (err) {
    showStatus(err.message || "Authentication failed.", "error");
  } finally {
    btn.disabled = false;
  }
});

$("btn-signout").addEventListener("click", async () => {
  await supabase.auth.signOut();
  showStatus("");
  renderView(null);
});

async function submitDecision(action) {
  const code = $("device-code").value.trim().toUpperCase();
  if (!code) {
    showStatus("Enter the device code shown in your terminal.", "error");
    return;
  }
  if (!currentSession?.access_token) {
    showStatus("Please sign in first.", "error");
    return;
  }

  $("btn-approve").disabled = true;
  $("btn-deny").disabled = true;
  showStatus(action === "approve" ? "Approving device…" : "Denying request…");

  try {
    const resp = await fetch(APPROVE_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${currentSession.access_token}`,
      },
      body: JSON.stringify({
        user_code: code,
        action,
      }),
    });
    const data = await resp.json().catch(() => ({}));
    if (!resp.ok) {
      throw new Error(data.message || data.error || `HTTP ${resp.status}`);
    }

    sessionStorage.removeItem(CODE_STORAGE_KEY);
    showStatus("");
    $("approve-view").classList.add("hidden");
    $("done-view").classList.remove("hidden");
    const box = $("done-box");
    if (action === "approve") {
      box.classList.remove("is-denied");
      $("done-title").textContent = "Device Approved";
      $("done-desc").textContent = "You're signed in! You can close this tab and return to your terminal.";
    } else {
      box.classList.add("is-denied");
      $("done-title").textContent = "Request Denied";
      $("done-desc").textContent = "The CLI login request was denied.";
    }
  } catch (err) {
    showStatus(`Error: ${err.message}`, "error");
    $("btn-approve").disabled = false;
    $("btn-deny").disabled = false;
  }
}

$("btn-approve").addEventListener("click", () => submitDecision("approve"));
$("btn-deny").addEventListener("click", () => submitDecision("deny"));

// Boot
initDeviceCode();
supabase.auth.onAuthStateChange((_event, session) => {
  renderView(session);
});
const { data } = await supabase.auth.getSession();
renderView(data.session);
