// Shared country detection, currency conversion (base MYR), and searchable country selector.

const STORAGE_COUNTRY_KEY = "openmi_country_v1";
const SESSION_GEO_KEY = "openmi_geo_country_v1";
const STORAGE_FX_KEY = "openmi_fx_rates_v1";
const FX_TTL_MS = 6 * 60 * 60 * 1000; // 6 hours
const FX_API_URL = "https://open.er-api.com/v6/latest/MYR";

// Compact [ISO2, Country Name, ISO4217 Currency] table covering ~195 countries & territories.
export const COUNTRIES = [
  ["AF", "Afghanistan", "AFN"],
  ["AL", "Albania", "ALL"],
  ["DZ", "Algeria", "DZD"],
  ["AD", "Andorra", "EUR"],
  ["AO", "Angola", "AOA"],
  ["AG", "Antigua and Barbuda", "XCD"],
  ["AR", "Argentina", "ARS"],
  ["AM", "Armenia", "AMD"],
  ["AU", "Australia", "AUD"],
  ["AT", "Austria", "EUR"],
  ["AZ", "Azerbaijan", "AZN"],
  ["BS", "Bahamas", "BSD"],
  ["BH", "Bahrain", "BHD"],
  ["BD", "Bangladesh", "BDT"],
  ["BB", "Barbados", "BBD"],
  ["BY", "Belarus", "BYN"],
  ["BE", "Belgium", "EUR"],
  ["BZ", "Belize", "BZD"],
  ["BJ", "Benin", "XOF"],
  ["BT", "Bhutan", "BTN"],
  ["BO", "Bolivia", "BOB"],
  ["BA", "Bosnia and Herzegovina", "BAM"],
  ["BW", "Botswana", "BWP"],
  ["BR", "Brazil", "BRL"],
  ["BN", "Brunei", "BND"],
  ["BG", "Bulgaria", "BGN"],
  ["BF", "Burkina Faso", "XOF"],
  ["BI", "Burundi", "BIF"],
  ["CV", "Cabo Verde", "CVE"],
  ["KH", "Cambodia", "KHR"],
  ["CM", "Cameroon", "XAF"],
  ["CA", "Canada", "CAD"],
  ["CF", "Central African Republic", "XAF"],
  ["TD", "Chad", "XAF"],
  ["CL", "Chile", "CLP"],
  ["CN", "China", "CNY"],
  ["CO", "Colombia", "COP"],
  ["KM", "Comoros", "KMF"],
  ["CG", "Congo", "XAF"],
  ["CD", "Congo (DRC)", "CDF"],
  ["CR", "Costa Rica", "CRC"],
  ["CI", "Côte d'Ivoire", "XOF"],
  ["HR", "Croatia", "EUR"],
  ["CU", "Cuba", "CUP"],
  ["CY", "Cyprus", "EUR"],
  ["CZ", "Czechia", "CZK"],
  ["DK", "Denmark", "DKK"],
  ["DJ", "Djibouti", "DJF"],
  ["DM", "Dominica", "XCD"],
  ["DO", "Dominican Republic", "DOP"],
  ["EC", "Ecuador", "USD"],
  ["EG", "Egypt", "EGP"],
  ["SV", "El Salvador", "USD"],
  ["GQ", "Equatorial Guinea", "XAF"],
  ["ER", "Eritrea", "ERN"],
  ["EE", "Estonia", "EUR"],
  ["SZ", "Eswatini", "SZL"],
  ["ET", "Ethiopia", "ETB"],
  ["FJ", "Fiji", "FJD"],
  ["FI", "Finland", "EUR"],
  ["FR", "France", "EUR"],
  ["GA", "Gabon", "XAF"],
  ["GM", "Gambia", "GMD"],
  ["GE", "Georgia", "GEL"],
  ["DE", "Germany", "EUR"],
  ["GH", "Ghana", "GHS"],
  ["GR", "Greece", "EUR"],
  ["GD", "Grenada", "XCD"],
  ["GT", "Guatemala", "GTQ"],
  ["GN", "Guinea", "GNF"],
  ["GW", "Guinea-Bissau", "XOF"],
  ["GY", "Guyana", "GYD"],
  ["HT", "Haiti", "HTG"],
  ["HN", "Honduras", "HNL"],
  ["HK", "Hong Kong", "HKD"],
  ["HU", "Hungary", "HUF"],
  ["IS", "Iceland", "ISK"],
  ["IN", "India", "INR"],
  ["ID", "Indonesia", "IDR"],
  ["IR", "Iran", "IRR"],
  ["IQ", "Iraq", "IQD"],
  ["IE", "Ireland", "EUR"],
  ["IL", "Israel", "ILS"],
  ["IT", "Italy", "EUR"],
  ["JM", "Jamaica", "JMD"],
  ["JP", "Japan", "JPY"],
  ["JO", "Jordan", "JOD"],
  ["KZ", "Kazakhstan", "KZT"],
  ["KE", "Kenya", "KES"],
  ["KI", "Kiribati", "AUD"],
  ["KP", "North Korea", "KPW"],
  ["KR", "South Korea", "KRW"],
  ["KW", "Kuwait", "KWD"],
  ["KG", "Kyrgyzstan", "KGS"],
  ["LA", "Laos", "LAK"],
  ["LV", "Latvia", "EUR"],
  ["LB", "Lebanon", "LBP"],
  ["LS", "Lesotho", "LSL"],
  ["LR", "Liberia", "LRD"],
  ["LY", "Libya", "LYD"],
  ["LI", "Liechtenstein", "CHF"],
  ["LT", "Lithuania", "EUR"],
  ["LU", "Luxembourg", "EUR"],
  ["MO", "Macao", "MOP"],
  ["MG", "Madagascar", "MGA"],
  ["MW", "Malawi", "MWK"],
  ["MY", "Malaysia", "MYR"],
  ["MV", "Maldives", "MVR"],
  ["ML", "Mali", "XOF"],
  ["MT", "Malta", "EUR"],
  ["MH", "Marshall Islands", "USD"],
  ["MR", "Mauritania", "MRU"],
  ["MU", "Mauritius", "MUR"],
  ["MX", "Mexico", "MXN"],
  ["FM", "Micronesia", "USD"],
  ["MD", "Moldova", "MDL"],
  ["MC", "Monaco", "EUR"],
  ["MN", "Mongolia", "MNT"],
  ["ME", "Montenegro", "EUR"],
  ["MA", "Morocco", "MAD"],
  ["MZ", "Mozambique", "MZN"],
  ["MM", "Myanmar", "MMK"],
  ["NA", "Namibia", "NAD"],
  ["NR", "Nauru", "AUD"],
  ["NP", "Nepal", "NPR"],
  ["NL", "Netherlands", "EUR"],
  ["NZ", "New Zealand", "NZD"],
  ["NI", "Nicaragua", "NIO"],
  ["NE", "Niger", "XOF"],
  ["NG", "Nigeria", "NGN"],
  ["MK", "North Macedonia", "MKD"],
  ["NO", "Norway", "NOK"],
  ["OM", "Oman", "OMR"],
  ["PK", "Pakistan", "PKR"],
  ["PW", "Palau", "USD"],
  ["PS", "Palestine", "ILS"],
  ["PA", "Panama", "PAB"],
  ["PG", "Papua New Guinea", "PGK"],
  ["PY", "Paraguay", "PYG"],
  ["PE", "Peru", "PEN"],
  ["PH", "Philippines", "PHP"],
  ["PL", "Poland", "PLN"],
  ["PT", "Portugal", "EUR"],
  ["PR", "Puerto Rico", "USD"],
  ["QA", "Qatar", "QAR"],
  ["RO", "Romania", "RON"],
  ["RU", "Russia", "RUB"],
  ["RW", "Rwanda", "RWF"],
  ["KN", "Saint Kitts and Nevis", "XCD"],
  ["LC", "Saint Lucia", "XCD"],
  ["VC", "Saint Vincent and the Grenadines", "XCD"],
  ["WS", "Samoa", "WST"],
  ["SM", "San Marino", "EUR"],
  ["ST", "Sao Tome and Principe", "STN"],
  ["SA", "Saudi Arabia", "SAR"],
  ["SN", "Senegal", "XOF"],
  ["RS", "Serbia", "RSD"],
  ["SC", "Seychelles", "SCR"],
  ["SL", "Sierra Leone", "SLE"],
  ["SG", "Singapore", "SGD"],
  ["SK", "Slovakia", "EUR"],
  ["SI", "Slovenia", "EUR"],
  ["SB", "Solomon Islands", "SBD"],
  ["SO", "Somalia", "SOS"],
  ["ZA", "South Africa", "ZAR"],
  ["SS", "South Sudan", "SSP"],
  ["ES", "Spain", "EUR"],
  ["LK", "Sri Lanka", "LKR"],
  ["SD", "Sudan", "SDG"],
  ["SR", "Suriname", "SRD"],
  ["SE", "Sweden", "SEK"],
  ["CH", "Switzerland", "CHF"],
  ["SY", "Syria", "SYP"],
  ["TW", "Taiwan", "TWD"],
  ["TJ", "Tajikistan", "TJS"],
  ["TZ", "Tanzania", "TZS"],
  ["TH", "Thailand", "THB"],
  ["TL", "Timor-Leste", "USD"],
  ["TG", "Togo", "XOF"],
  ["TO", "Tonga", "TOP"],
  ["TT", "Trinidad and Tobago", "TTD"],
  ["TN", "Tunisia", "TND"],
  ["TR", "Turkey", "TRY"],
  ["TM", "Turkmenistan", "TMT"],
  ["TV", "Tuvalu", "AUD"],
  ["UG", "Uganda", "UGX"],
  ["UA", "Ukraine", "UAH"],
  ["AE", "United Arab Emirates", "AED"],
  ["GB", "United Kingdom", "GBP"],
  ["US", "United States", "USD"],
  ["UY", "Uruguay", "UYU"],
  ["UZ", "Uzbekistan", "UZS"],
  ["VU", "Vanuatu", "VUV"],
  ["VA", "Vatican City", "EUR"],
  ["VE", "Venezuela", "VES"],
  ["VN", "Vietnam", "VND"],
  ["YE", "Yemen", "YER"],
  ["ZM", "Zambia", "ZMW"],
  ["ZW", "Zimbabwe", "ZWL"],
];

const COUNTRY_MAP = new Map(
  COUNTRIES.map(([code, name, currency]) => [code, { code, name, currency }])
);

// Static fallback rates per 1 MYR (used immediately before live FX loads or if offline).
const FALLBACK_RATES_MYR = {
  MYR: 1,
  USD: 0.225,
  EUR: 0.208,
  GBP: 0.177,
  SGD: 0.302,
  IDR: 3560,
  PHP: 13.1,
  THB: 7.75,
  VND: 5720,
  BND: 0.302,
  AUD: 0.345,
  NZD: 0.378,
  CAD: 0.315,
  INR: 19.0,
  JPY: 34.5,
  KRW: 314,
  CNY: 1.63,
  HKD: 1.75,
  TWD: 7.3,
  BRL: 1.3,
  MXN: 4.55,
  AED: 0.826,
  SAR: 0.844,
  CHF: 0.198,
  SEK: 2.42,
  NOK: 2.48,
  DKK: 1.55,
  PLN: 0.91,
  TRY: 7.75,
  ZAR: 4.08,
  NGN: 370,
  PKR: 62.5,
  BDT: 26.8,
  LKR: 66.0,
  NPR: 30.4,
  KES: 29.0,
  EGP: 11.1,
  ARS: 225,
  CLP: 218,
  COP: 990,
  PEN: 0.85,
  RUB: 22.0,
  UAH: 9.3,
  ILS: 0.84,
  QAR: 0.82,
  KWD: 0.069,
  BHD: 0.085,
  OMR: 0.087,
  CZK: 5.25,
  HUF: 84,
  RON: 1.03,
  MMK: 472,
  KHR: 915,
  LAK: 4950,
};

const ZERO_DECIMAL_CURRENCIES = new Set([
  "IDR", "JPY", "KRW", "VND", "CLP", "PYG", "UGX", "RWF",
  "ISK", "HUF", "TWD", "KHR", "LAK", "MMK", "GNF", "BIF", "DJF", "KMF", "VUV",
]);

const TIMEZONE_TO_COUNTRY = {
  "Asia/Kuala_Lumpur": "MY",
  "Asia/Kuching": "MY",
  "Asia/Singapore": "SG",
  "Asia/Jakarta": "ID",
  "Asia/Makassar": "ID",
  "Asia/Jayapura": "ID",
  "Asia/Pontianak": "ID",
  "Asia/Manila": "PH",
  "Asia/Bangkok": "TH",
  "Asia/Ho_Chi_Minh": "VN",
  "Asia/Saigon": "VN",
  "Asia/Brunei": "BN",
  "Asia/Tokyo": "JP",
  "Asia/Seoul": "KR",
  "Asia/Shanghai": "CN",
  "Asia/Hong_Kong": "HK",
  "Asia/Taipei": "TW",
  "Asia/Kolkata": "IN",
  "Asia/Calcutta": "IN",
  "Asia/Karachi": "PK",
  "Asia/Dhaka": "BD",
  "Asia/Dubai": "AE",
  "Asia/Riyadh": "SA",
  "Europe/London": "GB",
  "Europe/Paris": "FR",
  "Europe/Berlin": "DE",
  "Europe/Madrid": "ES",
  "Europe/Rome": "IT",
  "Europe/Amsterdam": "NL",
  "Europe/Zurich": "CH",
  "Europe/Stockholm": "SE",
  "Europe/Warsaw": "PL",
  "Europe/Istanbul": "TR",
  "Australia/Sydney": "AU",
  "Australia/Melbourne": "AU",
  "Australia/Brisbane": "AU",
  "Australia/Perth": "AU",
  "Pacific/Auckland": "NZ",
  "America/New_York": "US",
  "America/Chicago": "US",
  "America/Denver": "US",
  "America/Los_Angeles": "US",
  "America/Toronto": "CA",
  "America/Vancouver": "CA",
  "America/Sao_Paulo": "BR",
  "America/Mexico_City": "MX",
  "Africa/Lagos": "NG",
  "Africa/Johannesburg": "ZA",
  "Africa/Cairo": "EG",
};

export function countryFlag(iso2) {
  if (!iso2 || iso2.length !== 2) return "🌐";
  const upper = iso2.toUpperCase();
  const a = upper.charCodeAt(0) - 65 + 0x1f1e6;
  const b = upper.charCodeAt(1) - 65 + 0x1f1e6;
  if (a < 0x1f1e6 || b < 0x1f1e6) return "🌐";
  return String.fromCodePoint(a, b);
}

export function getCountry(code) {
  return COUNTRY_MAP.get(String(code || "").toUpperCase()) || COUNTRY_MAP.get("US");
}

function guessCountryFromBrowser() {
  try {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (tz && TIMEZONE_TO_COUNTRY[tz]) {
      return TIMEZONE_TO_COUNTRY[tz];
    }
  } catch {
    // ignore
  }
  try {
    const langs = navigator.languages?.length ? navigator.languages : [navigator.language];
    for (const lang of langs) {
      if (!lang) continue;
      const parts = String(lang).split(/[-_]/);
      if (parts.length >= 2) {
        const region = parts[parts.length - 1].toUpperCase();
        if (COUNTRY_MAP.has(region)) return region;
      }
    }
  } catch {
    // ignore
  }
  return "US";
}

function loadCachedFx() {
  try {
    const raw = localStorage.getItem(STORAGE_FX_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (parsed && parsed.rates && Date.now() - Number(parsed.ts || 0) < FX_TTL_MS) {
      return parsed.rates;
    }
  } catch {
    // ignore
  }
  return null;
}

function saveCachedFx(rates) {
  try {
    localStorage.setItem(STORAGE_FX_KEY, JSON.stringify({ ts: Date.now(), rates }));
  } catch {
    // ignore
  }
}

async function fetchJsonWithTimeout(url, timeoutMs = 3000) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const resp = await fetch(url, { signal: ctrl.signal });
    if (!resp.ok) return null;
    return await resp.json();
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

async function detectCountryByIp() {
  try {
    const cached = sessionStorage.getItem(SESSION_GEO_KEY);
    if (cached && COUNTRY_MAP.has(cached)) return cached;
  } catch {
    // ignore
  }

  let detected = null;
  const d1 = await fetchJsonWithTimeout("https://ipwho.is/", 2500);
  if (d1 && d1.success !== false && d1.country_code && COUNTRY_MAP.has(String(d1.country_code).toUpperCase())) {
    detected = String(d1.country_code).toUpperCase();
  } else {
    const d2 = await fetchJsonWithTimeout("https://api.country.is/", 2500);
    if (d2 && d2.country && COUNTRY_MAP.has(String(d2.country).toUpperCase())) {
      detected = String(d2.country).toUpperCase();
    }
  }

  if (detected) {
    try {
      sessionStorage.setItem(SESSION_GEO_KEY, detected);
    } catch {
      // ignore
    }
  }
  return detected;
}

// Singleton state shared across modules on the same page
const state = {
  countryCode: "MY",
  isManual: false,
  rates: { ...FALLBACK_RATES_MYR },
  ratesLive: false,
  listeners: new Set(),
  initialized: false,
};

function notify() {
  const info = getCurrencyState();
  for (const fn of state.listeners) {
    try {
      fn(info);
    } catch {
      // ignore
    }
  }
  window.dispatchEvent(new CustomEvent("openmi:country-change", { detail: info }));
}

export function getCurrencyState() {
  const country = getCountry(state.countryCode);
  return {
    countryCode: country.code,
    countryName: country.name,
    currency: country.currency,
    flag: countryFlag(country.code),
    isManual: state.isManual,
    rates: state.rates,
    ratesLive: state.ratesLive,
  };
}

export function setCountryManual(code) {
  const upper = String(code || "").toUpperCase();
  if (!COUNTRY_MAP.has(upper)) return;
  state.countryCode = upper;
  state.isManual = true;
  try {
    localStorage.setItem(STORAGE_COUNTRY_KEY, upper);
  } catch {
    // ignore
  }
  notify();
}

export async function resetCountryToAuto() {
  state.isManual = false;
  try {
    localStorage.removeItem(STORAGE_COUNTRY_KEY);
  } catch {
    // ignore
  }
  state.countryCode = guessCountryFromBrowser();
  notify();
  const ipCode = await detectCountryByIp();
  if (!state.isManual && ipCode && ipCode !== state.countryCode) {
    state.countryCode = ipCode;
    notify();
  }
}

export function subscribeCurrency(fn) {
  state.listeners.add(fn);
  return () => state.listeners.delete(fn);
}

export function initCurrency() {
  if (state.initialized) return getCurrencyState();
  state.initialized = true;

  // 1. Restore cached FX rates if fresh
  const cachedRates = loadCachedFx();
  if (cachedRates) {
    state.rates = { ...FALLBACK_RATES_MYR, ...cachedRates, MYR: 1 };
    state.ratesLive = true;
  }

  // 2. Restore manual override or instant browser guess
  let saved = null;
  try {
    saved = localStorage.getItem(STORAGE_COUNTRY_KEY);
  } catch {
    // ignore
  }
  if (saved && COUNTRY_MAP.has(saved.toUpperCase())) {
    state.countryCode = saved.toUpperCase();
    state.isManual = true;
  } else {
    state.countryCode = guessCountryFromBrowser();
    state.isManual = false;
  }

  // 3. Background IP geolocation (if not manually overridden)
  if (!state.isManual) {
    detectCountryByIp().then((ipCode) => {
      if (!state.isManual && ipCode && ipCode !== state.countryCode) {
        state.countryCode = ipCode;
        notify();
      }
    });
  }

  // 4. Background live FX fetch (if cache missing/stale)
  if (!cachedRates) {
    fetchJsonWithTimeout(FX_API_URL, 4000).then((data) => {
      if (data && data.result === "success" && data.rates) {
        state.rates = { ...FALLBACK_RATES_MYR, ...data.rates, MYR: 1 };
        state.ratesLive = true;
        saveCachedFx(data.rates);
        notify();
      }
    });
  }

  return getCurrencyState();
}

export function convertFromMyr(amountMyr, currencyCode) {
  const num = Number(amountMyr || 0);
  const curr = String(currencyCode || "MYR").toUpperCase();
  if (curr === "MYR") {
    return { amount: num, currency: "MYR", fallbackToUsd: false };
  }
  const rate = state.rates[curr];
  if (typeof rate === "number" && Number.isFinite(rate) && rate > 0) {
    return { amount: num * rate, currency: curr, fallbackToUsd: false };
  }
  // Fallback to USD if an exotic currency has no rate
  const usdRate = state.rates.USD || FALLBACK_RATES_MYR.USD;
  return { amount: num * usdRate, currency: "USD", fallbackToUsd: true };
}

export function formatMoney(amount, currencyCode) {
  const curr = String(currencyCode || "MYR").toUpperCase();
  const isZeroDec = ZERO_DECIMAL_CURRENCIES.has(curr);
  const value = isZeroDec ? Math.round(Number(amount || 0)) : Number(amount || 0);

  if (curr === "MYR") {
    return `MYR ${value.toFixed(2)}`;
  }

  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: curr,
      currencyDisplay: "narrowSymbol",
      minimumFractionDigits: isZeroDec ? 0 : 2,
      maximumFractionDigits: isZeroDec ? 0 : 2,
    }).format(value);
  } catch {
    return `${curr} ${isZeroDec ? value.toLocaleString() : value.toFixed(2)}`;
  }
}

export function formatFromMyr(amountMyr, currencyCode) {
  const conv = convertFromMyr(amountMyr, currencyCode);
  return {
    text: formatMoney(conv.amount, conv.currency),
    amount: conv.amount,
    currency: conv.currency,
    isMyr: conv.currency === "MYR",
  };
}

function escapeHtml(s) {
  return String(s ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export function mountCountrySelector(containerEl, onChange) {
  if (!containerEl) return () => {};
  initCurrency();

  const root = document.createElement("div");
  root.className = "country-picker";
  root.innerHTML = `
    <button type="button" class="country-picker-btn" aria-haspopup="listbox" aria-expanded="false" title="Select country &amp; currency">
      <span class="country-picker-flag" data-role="flag"></span>
      <span class="country-picker-code" data-role="code"></span>
      <span class="country-picker-caret" aria-hidden="true">▾</span>
    </button>
    <div class="country-picker-popover hidden" role="dialog" aria-label="Select country and currency">
      <div class="country-picker-header">
        <input type="search" class="country-picker-search" placeholder="Search country or currency…" autocomplete="off" />
        <div class="country-picker-meta">
          <span data-role="mode-label">Auto-detected</span>
          <button type="button" class="country-picker-reset hidden" data-role="reset">Auto-detect</button>
        </div>
      </div>
      <div class="country-picker-list" role="listbox"></div>
    </div>
  `;

  containerEl.innerHTML = "";
  containerEl.appendChild(root);

  const btn = root.querySelector(".country-picker-btn");
  const flagEl = root.querySelector('[data-role="flag"]');
  const codeEl = root.querySelector('[data-role="code"]');
  const popover = root.querySelector(".country-picker-popover");
  const searchInput = root.querySelector(".country-picker-search");
  const modeLabel = root.querySelector('[data-role="mode-label"]');
  const resetBtn = root.querySelector('[data-role="reset"]');
  const listEl = root.querySelector(".country-picker-list");

  let isOpen = false;

  function renderButton(info) {
    flagEl.textContent = info.flag;
    codeEl.textContent = `${info.countryCode} · ${info.currency}`;
    modeLabel.textContent = info.isManual ? "Manual selection" : "Auto-detected";
    resetBtn.classList.toggle("hidden", !info.isManual);
  }

  function renderList(query = "") {
    const q = query.trim().toLowerCase();
    const current = state.countryCode;
    const matches = q
      ? COUNTRIES.filter(
          ([code, name, curr]) =>
            name.toLowerCase().includes(q) ||
            code.toLowerCase().includes(q) ||
            curr.toLowerCase().includes(q)
        )
      : COUNTRIES;

    if (matches.length === 0) {
      listEl.innerHTML = `<div class="country-picker-empty">No countries match "${escapeHtml(query)}"</div>`;
      return;
    }

    listEl.innerHTML = matches
      .map(([code, name, curr]) => {
        const selected = code === current;
        return `
          <button type="button" class="country-picker-item${selected ? " is-selected" : ""}" role="option" aria-selected="${selected}" data-country="${code}">
            <span class="country-picker-item-left">
              <span class="country-picker-flag">${countryFlag(code)}</span>
              <span class="country-picker-item-name">${escapeHtml(name)}</span>
            </span>
            <span class="country-picker-item-curr">${escapeHtml(curr)}</span>
          </button>
        `;
      })
      .join("");
  }

  function setOpen(open) {
    isOpen = open;
    popover.classList.toggle("hidden", !open);
    btn.setAttribute("aria-expanded", String(open));
    if (open) {
      searchInput.value = "";
      renderList("");
      setTimeout(() => searchInput.focus(), 10);
      const selectedItem = listEl.querySelector(".is-selected");
      if (selectedItem) {
        selectedItem.scrollIntoView({ block: "nearest" });
      }
    }
  }

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    setOpen(!isOpen);
  });

  searchInput.addEventListener("input", () => {
    renderList(searchInput.value);
  });

  listEl.addEventListener("click", (e) => {
    const item = e.target.closest("[data-country]");
    if (!item) return;
    setCountryManual(item.getAttribute("data-country"));
    setOpen(false);
  });

  resetBtn.addEventListener("click", () => {
    resetCountryToAuto();
    setOpen(false);
  });

  const onDocClick = (e) => {
    if (isOpen && !root.contains(e.target)) {
      setOpen(false);
    }
  };
  const onKeyDown = (e) => {
    if (isOpen && e.key === "Escape") {
      setOpen(false);
      btn.focus();
    }
  };
  document.addEventListener("click", onDocClick);
  document.addEventListener("keydown", onKeyDown);

  renderButton(getCurrencyState());

  const unsub = subscribeCurrency((info) => {
    renderButton(info);
    if (isOpen) renderList(searchInput.value);
    if (typeof onChange === "function") onChange(info);
  });

  return () => {
    unsub();
    document.removeEventListener("click", onDocClick);
    document.removeEventListener("keydown", onKeyDown);
  };
}
