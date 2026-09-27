//! Server passwords in the keyring. Inside Flatpak oo7 uses an encrypted
//! file in the app's own data directory, unlocked through the Secret portal.

use crate::APP_ID;

/// Attributes that identify one saved password.
fn attributes<'a>(server: &'a str, user: &'a str) -> [(&'a str, &'a str); 3] {
    [("application", APP_ID), ("server", server), ("username", user)]
}

pub async fn save_password(server: &str, user: &str, password: &str) -> oo7::Result<()> {
    let keyring = oo7::Keyring::new().await?;
    keyring
        .create_item(
            &format!("Password for {user} on {server}"),
            &attributes(server, user),
            password,
            true,
        )
        .await?;
    Ok(())
}

pub async fn load_password(server: &str, user: &str) -> oo7::Result<Option<String>> {
    let keyring = oo7::Keyring::new().await?;
    let items = keyring.search_items(&attributes(server, user)).await?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    let secret = item.secret().await?;
    Ok(Some(String::from_utf8_lossy(secret.as_bytes()).into_owned()))
}
