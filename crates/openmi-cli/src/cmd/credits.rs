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
        println!("Plan:    {}", status.plan);
        println!("Credits: {}", status.balance);
    }
    Ok(())
}
