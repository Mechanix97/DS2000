//! Storage for the user's Discord credentials.
//!
//! DS-2000 ships no Discord credentials of its own. Discord restricts the `rpc` scope to an
//! application's owner plus a 50-slot tester list, and general approval is closed, so a single
//! shared application would cap the app at 50 users. Instead each user registers their own
//! Discord application and supplies its credentials — as the owner of that application they can
//! always authorise the scope.
//!
//! The client id is not secret (it is shown in Discord's own authorisation modal) and lives in
//! the plain config file. The client secret and the OAuth tokens are kept in the operating
//! system keyring, never on disk in the clear and never in the binary.

use keyring::Entry;
use thiserror::Error;

/// Service name under which every secret is filed in the OS keyring.
const KEYRING_SERVICE: &str = "DS2000";

/// Redirect URI the user must register under OAuth2 → Redirects in the Discord portal.
///
/// It is never actually navigated to: in the RPC flow the authorisation code arrives over the
/// local pipe, and this value only has to match a registered redirect for the token exchange to
/// be accepted. Keeping it fixed means the user copies one exact string instead of keeping a
/// third field in sync.
pub const DISCORD_REDIRECT_URI: &str = "http://localhost/";

#[derive(Error, Debug)]
pub enum CredentialError {
    /// The keyring is unreachable: locked session, denied access, or an unsupported platform.
    ///
    /// This is recoverable from the user's point of view — they can re-enter their credentials —
    /// so it must never be turned into a panic.
    #[error("could not access the system keyring: {0}")]
    Keyring(#[from] keyring::Error),
}

/// A secret held in the OS keyring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    ClientSecret,
    AccessToken,
    RefreshToken,
}

impl Secret {
    /// Key this secret is filed under. Stable: changing these strands existing installations.
    const fn key(self) -> &'static str {
        match self {
            Secret::ClientSecret => "discord_client_secret",
            Secret::AccessToken => "discord_access_token",
            Secret::RefreshToken => "discord_refresh_token",
        }
    }

    fn entry(self) -> Result<Entry, CredentialError> {
        Ok(Entry::new(KEYRING_SERVICE, self.key())?)
    }
}

/// Reads a secret, returning `Ok(None)` when it has never been stored.
///
/// A missing entry is a normal state — it is how a fresh installation looks — so it is not an
/// error.
pub fn read(secret: Secret) -> Result<Option<String>, CredentialError> {
    match secret.entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Stores a secret, replacing any previous value.
pub fn write(secret: Secret, value: &str) -> Result<(), CredentialError> {
    secret.entry()?.set_password(value)?;
    Ok(())
}

/// Removes a secret. Succeeds when there was nothing stored.
pub fn clear(secret: Secret) -> Result<(), CredentialError> {
    match secret.entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Removes every stored secret. Used when the user disconnects their Discord application.
pub fn clear_all() -> Result<(), CredentialError> {
    clear(Secret::ClientSecret)?;
    clear(Secret::AccessToken)?;
    clear(Secret::RefreshToken)?;
    Ok(())
}

/// Discards only the OAuth tokens, keeping the client secret.
///
/// Used when the stored tokens stop working: the application registration is still valid, so the
/// user should not have to paste their credentials again, only re-authorise.
pub fn clear_tokens() -> Result<(), CredentialError> {
    clear(Secret::AccessToken)?;
    clear(Secret::RefreshToken)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_keys_are_distinct() {
        let keys = [
            Secret::ClientSecret.key(),
            Secret::AccessToken.key(),
            Secret::RefreshToken.key(),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "two secrets share a keyring key");
            }
        }
    }

    #[test]
    fn redirect_uri_matches_what_the_guide_tells_users_to_register() {
        // The setup guide instructs users to paste this exact string into the Discord portal.
        // If it changes, every existing installation breaks with `invalid_redirect_uri`.
        assert_eq!(DISCORD_REDIRECT_URI, "http://localhost/");
    }
}
