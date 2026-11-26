use openmi_core::fetch_models_with_fallback;
use openmi_platform::AppPaths;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenStore};
use crate::exit_codes::CliError;

pub async fn execute(json: bool, api_url: Option<String>) -> Result<(), CliError> {
    let store = default_token_store()?;
    let stored = store.load()?;
    let base_url = resolve_api_base_url(api_url.as_deref(), stored.as_ref());

    let bearer = if stored.is_some() {
        let client = DeviceAuthClient::new(&base_url)?;
        client
            .ensure_valid_tokens(&store)
            .await
            .ok()
            .map(|t| t.access_token)
    } else {
        None
    };

    let endpoint = format!("{base_url}/functions/v1/models");
    let cache_path = AppPaths::resolve().ok().map(|p| p.models_cache_file());
    let models =
        fetch_models_with_fallback(Some(&endpoint), bearer.as_deref(), cache_path.as_deref()).await;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&models).map_err(anyhow::Error::from)?
        );
    } else {
        for m in models {
            let tier = if m.paid_only { "[Paid]" } else { "Free" };
            println!(
                "{:<28} {:<24} {:>3} cr/hr  {:<6}  {}k ctx",
                m.id,
                m.name(),
                m.credit_cost,
                tier,
                m.context_window / 1000
            );
        }
    }

    Ok(())
}
