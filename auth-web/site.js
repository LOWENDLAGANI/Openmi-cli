(() => {
  const REF_STORAGE_KEY = "openmi_pending_ref_code";

  // Preserve legacy `/?code=XXXX-XXXX` device-login links by forwarding to `/verify`.
  const params = new URLSearchParams(window.location.search);
  if (params.get("code")) {
    window.location.replace(`/verify${window.location.search}`);
    return;
  }
  const ref = (params.get("ref") || "").trim().toUpperCase();
  if (ref) {
    try {
      localStorage.setItem(REF_STORAGE_KEY, ref);
    } catch (_) {}
  }

  // Mobile menu toggle
  const menuToggle = document.getElementById("menu-toggle");
  const mobileDrawer = document.getElementById("mobile-drawer");
  if (menuToggle && mobileDrawer) {
    menuToggle.addEventListener("click", () => {
      const open = mobileDrawer.classList.toggle("is-open");
      menuToggle.setAttribute("aria-expanded", String(open));
    });
  }

  // One-click copy for command boxes
  async function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    document.body.removeChild(ta);
  }

  document.querySelectorAll("[data-copy]").forEach((btn) => {
    const labelEl = btn.querySelector("[data-copy-label]");
    const defaultLabel = labelEl ? labelEl.textContent : "Copy";
    let timer = null;

    btn.addEventListener("click", async () => {
      const value = btn.getAttribute("data-copy") || "";
      if (!value) return;
      try {
        await copyText(value);
        btn.classList.add("is-copied");
        if (labelEl) labelEl.textContent = "Copied";
        clearTimeout(timer);
        timer = setTimeout(() => {
          btn.classList.remove("is-copied");
          if (labelEl) labelEl.textContent = defaultLabel;
        }, 1800);
      } catch (_) {}
    });
  });

  // Landing page: interactive TUI preview tabs
  const previewTabs = document.querySelectorAll("[data-preview-tab]");
  const previewPanels = document.querySelectorAll("[data-preview-panel]");
  if (previewTabs.length && previewPanels.length) {
    previewTabs.forEach((tab) => {
      tab.addEventListener("click", () => {
        const target = tab.getAttribute("data-preview-tab");
        previewTabs.forEach((t) => {
          const active = t === tab;
          t.classList.toggle("is-active", active);
          t.setAttribute("aria-selected", String(active));
        });
        previewPanels.forEach((panel) => {
          panel.classList.toggle(
            "is-active",
            panel.getAttribute("data-preview-panel") === target
          );
        });
      });
    });
  }

  // Download page: terminal / IDE quick tips
  const ideTips = {
    vscode:
      'Press <kbd>Ctrl</kbd> + <kbd>`</kbd> (or <kbd>Cmd</kbd> + <kbd>J</kbd> on macOS) to open the integrated terminal in VS Code.',
    cursor:
      'Press <kbd>Ctrl</kbd> + <kbd>`</kbd> in Cursor to open the integrated terminal alongside your editor.',
    termux:
      'Open <strong>Termux</strong> on Android (<code>aarch64</code>). Openmi ships a native rustls binary with zero OpenSSL dependency.',
    wt:
      'Open <strong>Windows Terminal</strong> or PowerShell. Both <code>npm install -g openmi</code> and the PowerShell installer work natively.',
    shell:
      'Open <strong>Terminal</strong>, iTerm2, Ghostty, or your Linux shell. Works locally and over SSH.',
    intellij:
      'Press <kbd>Alt</kbd> + <kbd>F12</kbd> (Windows/Linux) or <kbd>Option</kbd> + <kbd>F12</kbd> (macOS) to open the terminal in JetBrains IDEs.',
  };

  const ideButtons = document.querySelectorAll("[data-ide]");
  const ideTipBox = document.getElementById("ide-tip-box");
  if (ideButtons.length && ideTipBox) {
    ideButtons.forEach((btn) => {
      btn.addEventListener("click", () => {
        const key = btn.getAttribute("data-ide");
        ideButtons.forEach((b) => {
          const active = b === btn;
          b.classList.toggle("is-active", active);
          b.setAttribute("aria-pressed", String(active));
        });
        if (ideTips[key]) {
          ideTipBox.innerHTML = ideTips[key];
        }
      });
    });
  }

  // Download page: install method tabs
  const methodTabs = document.querySelectorAll("[data-method-tab]");
  const methodPanels = document.querySelectorAll("[data-method-panel]");
  if (methodTabs.length && methodPanels.length) {
    methodTabs.forEach((tab) => {
      tab.addEventListener("click", () => {
        const target = tab.getAttribute("data-method-tab");
        methodTabs.forEach((t) => {
          const active = t === tab;
          t.classList.toggle("is-active", active);
          t.setAttribute("aria-selected", String(active));
        });
        methodPanels.forEach((panel) => {
          panel.classList.toggle(
            "is-active",
            panel.getAttribute("data-method-panel") === target
          );
        });
      });
    });
  }
})();
