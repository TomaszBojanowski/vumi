//! Server passwords in the keyring. Inside Flatpak oo7 uses an encrypted
//! file in the app's own data directory, unlocked through the Secret portal.

use crate::APP_ID;

/// oo7's error is large; boxed, every result of these functions stays small.
type Result<T> = std::result::Result<T, Box<oo7::Error>>;

/// Attributes that identify one saved password.
fn attributes<'a>(server: &'a str, user: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("application", APP_ID),
        ("server", server),
        ("username", user),
    ]
}

pub async fn save_password(server: &str, user: &str, password: &str) -> Result<()> {
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

pub async fn load_password(server: &str, user: &str) -> Result<Option<String>> {
    let keyring = oo7::Keyring::new().await?;
    let items = keyring.search_items(&attributes(server, user)).await?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    let secret = item.secret().await?;
    Ok(Some(
        String::from_utf8_lossy(secret.as_bytes()).into_owned(),
    ))
}

/// Removes the saved password (logging out).
pub async fn delete_password(server: &str, user: &str) -> Result<()> {
    let keyring = oo7::Keyring::new().await?;
    keyring.delete(&attributes(server, user)).await?;
    Ok(())
}
