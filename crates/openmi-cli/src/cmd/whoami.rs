use anyhow::Result;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, AuthError, TokenStore};

pub async fn execute(api_url: Option<String>) -> Result<()> {
    let store = default_token_store()?;
    let stored = store.load()?.ok_or(AuthError::NotLoggedIn)?;
    let base_url = resolve_api_base_url(api_url.as_deref(), Some(&stored));
    let client = DeviceAuthClient::new(&base_url)?;

    let valid = client.ensure_valid_tokens(&store).await?;
    let profile = match client.whoami(&valid.access_token).await {
        Ok(p) => p,
        Err(AuthError::Revoked) if !valid.refresh_token.is_empty() => {
            // Try one refresh in case the access token was invalidated early.
            let refreshed = client.refresh_tokens(&valid).await?;
            store.save(&refreshed)?;
            client.whoami(&refreshed.access_token).await?
        }
        Err(err) => return Err(err.into()),
    };

    println!("User ID: {}", profile.id);
    if let Some(email) = profile.email {
        println!("Email:   {email}");
    }
    if let Some(plan) = profile.plan {
        println!("Plan:    {plan}");
    }
    println!("API URL: {base_url}");
    Ok(())
}
