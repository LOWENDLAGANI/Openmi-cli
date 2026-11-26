use anyhow::Result;

use crate::auth::device_flow::{resolve_api_base_url, DeviceAuthClient};
use crate::auth::{default_token_store, TokenSet, TokenStore};

pub async fn execute(
    token: Option<String>,
    refresh_token: Option<String>,
    api_url: Option<String>,
) -> Result<()> {
    let store = default_token_store()?;
    let existing = store.load()?;
    let base_url = resolve_api_base_url(api_url.as_deref(), existing.as_ref());

    // Fallback paste-token login for headless or offline environments.
    if let Some(access_token) = token {
        let tokens = TokenSet {
            access_token,
            refresh_token: refresh_token.unwrap_or_default(),
            expires_at: 0,
            user_id: None,
            email: None,
            api_base_url: Some(base_url.clone()),
        };
        store.save(&tokens)?;
        println!(
            "Saved token to {} (mode 0600 on Unix).",
            store.path().display()
        );
        return Ok(());
    }

    let client = DeviceAuthClient::new(&base_url)?;
    let start = client.start_device_flow().await?;

    println!("To sign in to Openmi, open this URL in any browser:");
    println!("  {}", start.verify_url);
    println!();
    println!("And confirm this one-time code:");
    println!("  {}", start.user_code);
    println!();
    println!("Waiting for approval...");

    let tokens = client.poll_until_approved(&start).await?;
    store.save(&tokens)?;

    match tokens.email.as_deref() {
        Some(email) => println!("Logged in as {email}."),
        None => println!("Logged in successfully."),
    }
    Ok(())
}
