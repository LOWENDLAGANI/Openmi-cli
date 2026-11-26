use openmi_core::catalog::CatalogError;
use openmi_core::fetch_credit_status;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, AuthError, TokenStore};
use crate::exit_codes::CliError;

pub async fn execute(json: bool, api_url: Option<String>) -> Result<(), CliError> {
    let store = default_token_store()?;
    let stored = store.load()?.ok_or(AuthError::NotLoggedIn)?;
    let base_url = resolve_api_base_url(api_url.as_deref(), Some(&stored));
    let client = DeviceAuthClient::new(&base_url)?;
    let valid = client.ensure_valid_tokens(&store).await?;

    let endpoint = format!("{base_url}/functions/v1/credits");
    let status = match fetch_credit_status(&endpoint, &valid.access_token).await {
        Ok(s) => s,
        Err(CatalogError::Unauthorized) => return Err(CliError::Auth(AuthError::Revoked)),
        Err(err) => return Err(CliError::Other(err.into())),
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(anyhow::Error::from)?
        );
    } else {
        let daily_cap = status
            .daily_limit
            .unwrap_or(if status.is_paid { 15 } else { 6 });
        if status.is_paid {
            if let Some(ref until) = status.paid_until {
                println!(
                    "Plan:          PAID (until {})",
                    until.get(..10).unwrap_or(until)
                );
            } else {
                println!("Plan:          PAID");
            }
        } else {
            println!("Plan:          FREE");
        }
        println!(
            "Daily Credits: {} / {} (resets 00:00 UTC)",
            status.balance, daily_cap
        );
        println!("Wallet Credit: {} (never expires)", status.wallet_credit);
        if let Some((model, mins)) = status.active_pass_remaining() {
            println!("Active Pass:   {} ({}m remaining)", model, mins);
        } else {
            println!("Active Pass:   none (1-hr pass starts on your first prompt)");
        }
        if !status.is_paid {
            println!("Upgrade:       {}", status.effective_upgrade_url());
        }
    }
    Ok(())
}
