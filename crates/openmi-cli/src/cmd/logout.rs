use anyhow::Result;

use crate::auth::{default_token_store, TokenStore};

pub async fn execute() -> Result<()> {
    let store = default_token_store()?;
    store.clear()?;
    println!("Logged out (cleared stored credentials).");
    Ok(())
}
