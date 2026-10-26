//! Account received from the launcher and online login (`docs/specs/authentication.md` § 2, § 5).
//!
//! Like the official game, Rustcraft never does the Microsoft/Xbox chain: the launcher passes
//! it the username, UUID and access token as arguments. The token lives only in memory, is
//! never logged (its `Debug` is masked) and only goes to the session services.

use std::sync::OnceLock;

/// Minecraft access token (Bearer). No `Display`, masked `Debug`, wiped on release.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    /// Usable token, or `None` for the filler values of offline launchers
    /// (`0`, `null`, empty string…): a real token is a JWT of several hundred bytes.
    pub fn new(value: String) -> Option<AccessToken> {
        let usable = value.len() >= 16 && !value.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control());
        usable.then_some(AccessToken(value))
    }

    /// Raw value, only for the header or body of a request to the services.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessToken(<masked>)")
    }
}

impl Drop for AccessToken {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|b| *b = 0);
        std::hint::black_box(&bytes);
    }
}

/// Player account as passed by the launcher (`--username`, `--uuid`, `--accessToken`,
/// `--userType`, `--xuid`, `--clientId`).
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    /// Profile UUID; the offline one of the username if the launcher gives no valid one.
    pub uuid: u128,
    pub token: Option<AccessToken>,
    /// `msa`, `legacy`, `mojang`… (informational; the 26.3 game no longer reads this option).
    pub user_type: Option<String>,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("name", &self.name)
            .field("uuid", &format_args!("{:032x}", self.uuid))
            .field("token", &self.token.as_ref().map(|_| "<masked>"))
            .field("user_type", &self.user_type)
            .finish_non_exhaustive()
    }
}

static ACCOUNT: OnceLock<Account> = OnceLock::new();

/// Launcher account, set once at startup.
pub fn set_account(account: Account) {
    let _ = ACCOUNT.set(account);
}

/// Launcher account, if there is one.
pub fn account() -> Option<&'static Account> {
    ACCOUNT.get()
}

/// UUID read like the game: 32 hexadecimal digits, with or without dashes.
pub fn parse_uuid(s: &str) -> Option<u128> {
    let hex: String = s.chars().filter(|&c| c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

/// Trace of the online login steps (`RUSTCRAFT_AUTHLOG=1`): never any secret.
pub fn authlog(step: impl std::fmt::Display) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_AUTHLOG").is_some_and(|v| v == "1")) {
        eprintln!("auth: {step}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_masked_and_placeholders_rejected() {
        let t = AccessToken::new("eyJhbGciOiJIUzI1NiJ9.secret-part".into()).expect("token");
        assert!(!format!("{t:?}").contains("secret"));
        assert_eq!(AccessToken::new("0".into()), None);
        assert_eq!(AccessToken::new(String::new()), None);
        let a = Account {
            name: "Bob".into(),
            uuid: 1,
            token: Some(t),
            user_type: None,
            xuid: Some("x".into()),
            client_id: None,
        };
        assert!(!format!("{a:?}").contains("secret"));
    }

    #[test]
    fn uuid_with_or_without_dashes() {
        let v = 0x069a79f444e94726a5befca90e38aaf5u128;
        assert_eq!(parse_uuid("069a79f444e94726a5befca90e38aaf5"), Some(v));
        assert_eq!(parse_uuid("069a79f4-44e9-4726-a5be-fca90e38aaf5"), Some(v));
        assert_eq!(parse_uuid("not-a-uuid"), None);
    }
}
