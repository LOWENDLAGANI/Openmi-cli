use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("unauthorized or expired token")]
    Unauthorized,

    #[error("unexpected status code: {0}")]
    UnexpectedStatus(u16),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

fn default_credit_cost() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub context_window: u32,
    #[serde(default = "default_credit_cost")]
    pub credit_cost: u32,
    #[serde(default)]
    pub paid_only: bool,
    /// User-facing name configured in the Admin Panel (falls back to `label`, then `id`).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Optional short description shown in the `/model` selector.
    #[serde(default)]
    pub description: Option<String>,
    /// True when this model comes from the user's own BYOK provider configured at `/provider`.
    #[serde(default)]
    pub is_byok: bool,
    /// Optional provider display name for BYOK models (e.g. `OpenRouter`, `OpenAI`).
    #[serde(default)]
    pub provider_name: Option<String>,
}

impl ModelInfo {
    /// Name to show users: `display_name`, else `label`, else `id`.
    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| Some(self.label.trim()).filter(|s| !s.is_empty()))
            .unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditStatus {
    pub balance: i64,
    pub plan: String,
    #[serde(default)]
    pub wallet_credit: i64,
    #[serde(default)]
    pub daily_limit: Option<i64>,
    #[serde(default)]
    pub daily_tokens_used: i64,
    #[serde(default)]
    pub daily_token_limit: Option<i64>,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub is_paid: bool,
    #[serde(default)]
    pub paid_until: Option<String>,
    #[serde(default)]
    pub next_reset_at: Option<String>,
    #[serde(default)]
    pub active_model: Option<String>,
    #[serde(default)]
    pub pass_expires_at: Option<String>,
    #[serde(default)]
    pub payment_link: Option<String>,
    #[serde(default)]
    pub pay_url: Option<String>,
    #[serde(default)]
    pub provider_url: Option<String>,
    #[serde(default)]
    pub topup_paused: bool,
}

impl CreditStatus {
    /// Returns the active model and remaining whole minutes (rounded up) if a server pass is live.
    pub fn active_pass_remaining(&self) -> Option<(&str, u64)> {
        let model = self.active_model.as_deref()?;
        let exp = self.pass_expires_at.as_deref()?;
        let mins = remaining_minutes_from_rfc3339(exp)?;
        Some((model, mins))
    }

    /// Effective per-prompt agent loop step/turn limit (defaults: 40 Free, 70 Paid).
    pub fn effective_max_steps(&self) -> u32 {
        self.max_steps
            .filter(|&v| v >= 1)
            .unwrap_or(if self.is_paid { 70 } else { 40 })
    }

    /// Effective daily input+output token hard cap (defaults: 2,000,000 Free, 7,000,000 Paid).
    pub fn effective_daily_token_limit(&self) -> i64 {
        self.daily_token_limit
            .unwrap_or(if self.is_paid { 7_000_000 } else { 2_000_000 })
    }

    pub fn effective_upgrade_url(&self) -> &str {
        self.pay_url
            .as_deref()
            .or(self.payment_link.as_deref())
            .unwrap_or("https://sociabuzz.com/minetallestmc/tribe")
    }

    /// Resolve the web console `/provider` URL for managing BYOK API keys.
    pub fn effective_provider_url(&self) -> Option<String> {
        if let Some(u) = self
            .provider_url
            .as_deref()
            .map(str::trim)
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        {
            return Some(u.to_string());
        }
        // Derive `/provider` from `pay_url` when it points to the Openmi web console (`/pay`).
        if let Some(pay) = self.pay_url.as_deref().map(str::trim) {
            if let Some(prefix) = pay.strip_suffix("/pay") {
                return Some(format!("{prefix}/provider"));
            }
        }
        None
    }
}

/// Format a token count concisely (e.g. `2.00M`, `450.0k`, `850`).
pub fn format_token_count(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Parse an RFC3339 UTC timestamp (e.g. `2026-11-26T09:30:00Z` or `+00:00`) into Unix seconds.
pub fn parse_rfc3339_utc_secs(iso: &str) -> Option<i64> {
    let s = iso.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil algorithm (UTC proleptic Gregorian calendar).
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = month as i64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + min * 60 + sec)
}

/// Return remaining minutes (rounded up, min 1) until `iso`, or `None` if already expired.
pub fn remaining_minutes_from_rfc3339(iso: &str) -> Option<u64> {
    let target = parse_rfc3339_utc_secs(iso)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let diff = target - now;
    if diff <= 0 {
        None
    } else {
        Some((diff as u64).div_ceil(60))
    }
}

/// Static fallback model list used when the remote catalog and local cache are unavailable.
pub fn fallback_models() -> Vec<ModelInfo> {
    vec![ModelInfo {
        id: "openmi-default".to_string(),
        label: "Openmi Default".to_string(),
        context_window: 128_000,
        credit_cost: 3,
        paid_only: false,
        display_name: None,
        description: None,
        is_byok: false,
        provider_name: None,
    }]
}

/// Fetch models from `endpoint` (via rustls), updating `cache_path` on success, or falling back
/// to the cached file / static defaults on network failure.
pub async fn fetch_models_with_fallback(
    endpoint: Option<&str>,
    bearer_token: Option<&str>,
    cache_path: Option<&Path>,
) -> Vec<ModelInfo> {
    if let Some(url) = endpoint {
        if let Ok(models) = fetch_models_remote(url, bearer_token).await {
            if let Some(path) = cache_path {
                if let Ok(bytes) = serde_json::to_vec_pretty(&models) {
                    let _ = openmi_platform::atomic_write(path, &bytes);
                }
            }
            return models;
        }
    }

    if let Some(path) = cache_path {
        if let Ok(bytes) = fs::read(path) {
            if let Ok(cached) = serde_json::from_slice::<Vec<ModelInfo>>(&bytes) {
                if !cached.is_empty() {
                    return cached;
                }
            }
        }
    }

    fallback_models()
}

async fn fetch_models_remote(
    url: &str,
    bearer_token: Option<&str>,
) -> Result<Vec<ModelInfo>, CatalogError> {
    let client = reqwest::Client::builder().build()?;
    let mut req = client.get(url);
    if let Some(tok) = bearer_token {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await?;
    if resp.status().as_u16() == 401 {
        return Err(CatalogError::Unauthorized);
    }
    if !resp.status().is_success() {
        return Err(CatalogError::UnexpectedStatus(resp.status().as_u16()));
    }
    Ok(resp.json::<Vec<ModelInfo>>().await?)
}

/// Fetch credit balance from the managed proxy endpoint (rustls-only).
pub async fn fetch_credit_status(
    endpoint: &str,
    bearer_token: &str,
) -> Result<CreditStatus, CatalogError> {
    let client = reqwest::Client::builder().build()?;
    let resp = client
        .get(endpoint)
        .bearer_auth(bearer_token)
        .send()
        .await?;
    if resp.status().as_u16() == 401 {
        return Err(CatalogError::Unauthorized);
    }
    if !resp.status().is_success() {
        return Err(CatalogError::UnexpectedStatus(resp.status().as_u16()));
    }
    Ok(resp.json::<CreditStatus>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_models_uses_cache_when_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("models.json");
        let cached = vec![ModelInfo {
            id: "cached-model".to_string(),
            label: "Cached Model".to_string(),
            context_window: 64_000,
            credit_cost: 3,
            paid_only: true,
            display_name: None,
            description: None,
            is_byok: false,
            provider_name: None,
        }];
        fs::write(&cache, serde_json::to_vec(&cached).unwrap()).unwrap();

        let models = fetch_models_with_fallback(None, None, Some(&cache)).await;
        assert_eq!(models, cached);
    }

    #[test]
    fn model_name_prefers_display_name_then_label_then_id() {
        let mut m = ModelInfo {
            id: "deepseek-chat".into(),
            label: "DS".into(),
            context_window: 64_000,
            credit_cost: 2,
            paid_only: false,
            display_name: Some("  DeepSeek Fast ".into()),
            description: None,
            is_byok: false,
            provider_name: None,
        };
        assert_eq!(m.name(), "DeepSeek Fast");
        m.display_name = None;
        assert_eq!(m.name(), "DS");
        m.label.clear();
        assert_eq!(m.name(), "deepseek-chat");
    }

    #[tokio::test]
    async fn fetch_models_uses_static_fallback_without_cache() {
        let models = fetch_models_with_fallback(None, None, None).await;
        assert_eq!(models, fallback_models());
    }

    #[test]
    fn deserializes_legacy_and_extended_payloads() {
        let legacy_model: ModelInfo =
            serde_json::from_str(r#"{"id":"m1","label":"M1","context_window":128000}"#).unwrap();
        assert_eq!(legacy_model.credit_cost, 3);
        assert!(!legacy_model.paid_only);
        assert!(!legacy_model.is_byok);
        assert_eq!(legacy_model.provider_name, None);

        let byok_model: ModelInfo = serde_json::from_str(
            r#"{"id":"byok:11111111-1111-1111-1111-111111111111:gpt-4o","label":"GPT-4o","context_window":128000,"credit_cost":0,"paid_only":false,"is_byok":true,"provider_name":"OpenAI"}"#,
        )
        .unwrap();
        assert!(byok_model.is_byok);
        assert_eq!(byok_model.credit_cost, 0);
        assert_eq!(byok_model.provider_name.as_deref(), Some("OpenAI"));

        let legacy_credit: CreditStatus =
            serde_json::from_str(r#"{"balance":6,"plan":"free","pay_url":"https://openmi.app/pay"}"#)
                .unwrap();
        assert_eq!(legacy_credit.wallet_credit, 0);
        assert!(!legacy_credit.is_paid);
        assert_eq!(legacy_credit.effective_max_steps(), 40);
        assert_eq!(legacy_credit.effective_daily_token_limit(), 2_000_000);
        assert_eq!(
            legacy_credit.effective_provider_url().as_deref(),
            Some("https://openmi.app/provider")
        );

        let paid_credit: CreditStatus = serde_json::from_str(
            r#"{"balance":15,"plan":"paid","is_paid":true,"daily_tokens_used":1250000,"daily_token_limit":7000000,"max_steps":70,"provider_url":"https://console.openmi.dev/provider"}"#,
        )
        .unwrap();
        assert_eq!(paid_credit.daily_tokens_used, 1_250_000);
        assert_eq!(paid_credit.effective_daily_token_limit(), 7_000_000);
        assert_eq!(paid_credit.effective_max_steps(), 70);
        assert_eq!(
            paid_credit.effective_provider_url().as_deref(),
            Some("https://console.openmi.dev/provider")
        );

        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(12_500), "12.5k");
        assert_eq!(format_token_count(2_000_000), "2.00M");
        assert_eq!(format_token_count(7_000_000), "7.00M");

        assert_eq!(parse_rfc3339_utc_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_utc_secs("2026-11-26T00:00:00+00:00"),
            Some(1_795_651_200)
        );
    }
}
