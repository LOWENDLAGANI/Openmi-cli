use openmi_core::catalog::CatalogError;
use openmi_core::fetch_credit_status;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, AuthError, TokenStore};
use crate::browser;
use crate::exit_codes::CliError;

pub async fn execute(api_url: Option<String>, no_browser: bool) -> Result<(), CliError> {
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

    let upgrade_url = status.effective_upgrade_url().to_string();
    if !no_browser && browser::open_url(&upgrade_url) {
        println!("Opened upgrade page in your browser:");
        println!("  {upgrade_url}");
    } else {
        println!("Open this URL in your browser to upgrade your Openmi pass:");
        println!("  {upgrade_url}");
    }
    Ok(())
}
