use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub context_window: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditStatus {
    pub balance: i64,
    pub plan: String,
}

/// Static fallback model list used when the remote catalog and local cache are unavailable.
pub fn fallback_models() -> Vec<ModelInfo> {
    vec![ModelInfo {
        id: "openmi-default".to_string(),
        label: "Openmi Default".to_string(),
        context_window: 128_000,
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
        }];
        fs::write(&cache, serde_json::to_vec(&cached).unwrap()).unwrap();

        let models = fetch_models_with_fallback(None, None, Some(&cache)).await;
        assert_eq!(models, cached);
    }

    #[tokio::test]
    async fn fetch_models_uses_static_fallback_without_cache() {
        let models = fetch_models_with_fallback(None, None, None).await;
        assert_eq!(models, fallback_models());
    }
}
