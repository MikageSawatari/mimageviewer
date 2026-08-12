use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use hmac::{Hmac, Mac};
use mimageviewer_ipc::{AuthRecord, production_argon2, validate_record};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const COOKIE_NAME: &str = "miv_remote_session";
const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;
const LOCKOUT_THRESHOLD: u32 = 5;
const FIRST_LOCKOUT: Duration = Duration::from_secs(30);
const MAX_LOCKOUT_EXPONENT: u32 = 10;
const SESSION_MAX_AGE_SECONDS: u64 = 90 * 24 * 60 * 60;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct AuthToken {
    printable: String,
    expected_ascii: [u8; TOKEN_HEX_LEN],
}

pub struct AuthService {
    bearer: AuthToken,
    pin_hash: String,
    session_secret: [u8; TOKEN_BYTES],
    lockout: Mutex<LockoutState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthSessionIdentity([u8; TOKEN_BYTES]);

impl AuthSessionIdentity {
    pub fn fallback_client_id(self) -> String {
        format!("cookie-{}", encode_hex(&self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Bearer,
    SessionCookie(AuthSessionIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthDecision {
    Authorized(AuthSource),
    Unauthorized,
}

#[derive(Default)]
pub struct AuthInput<'a> {
    pub authorization: Option<&'a str>,
    pub cookie: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinVerification {
    Success,
    Invalid {
        failure_count: u32,
        lockout: Option<Duration>,
    },
    Locked {
        failure_count: u32,
        remaining: Duration,
    },
}

pub struct SessionCookie {
    pub header: String,
    pub sensitive_value: String,
}

#[derive(Default)]
struct LockoutState {
    failure_count: u32,
    locked_until: Option<Instant>,
}

impl AuthToken {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut raw = [0_u8; TOKEN_BYTES];
        getrandom::fill(&mut raw)?;
        Ok(Self::from_raw(raw))
    }

    #[cfg(test)]
    pub fn from_printable_for_test(printable: &str) -> Self {
        assert_eq!(printable.len(), TOKEN_HEX_LEN);
        let mut expected_ascii = [0_u8; TOKEN_HEX_LEN];
        expected_ascii.copy_from_slice(printable.as_bytes());
        Self {
            printable: printable.to_owned(),
            expected_ascii,
        }
    }

    fn from_raw(raw: [u8; TOKEN_BYTES]) -> Self {
        let expected_ascii: [u8; TOKEN_HEX_LEN] = encode_hex(&raw)
            .into_bytes()
            .try_into()
            .expect("fixed hex length");
        let printable = String::from_utf8(expected_ascii.to_vec())
            .expect("hex token generation only emits ASCII");
        Self {
            printable,
            expected_ascii,
        }
    }

    pub fn printable(&self) -> &str {
        &self.printable
    }

    fn matches(&self, candidate: &str) -> bool {
        let bytes = candidate.as_bytes();
        let mut padded = [0_u8; TOKEN_HEX_LEN];
        for (dst, src) in padded.iter_mut().zip(bytes.iter().copied()) {
            *dst = src;
        }
        let content_match = self.expected_ascii.ct_eq(&padded);
        let length_match = (bytes.len() as u64).ct_eq(&(TOKEN_HEX_LEN as u64));
        bool::from(content_match & length_match)
    }
}

impl AuthService {
    pub fn new(record: AuthRecord, bearer: AuthToken) -> Result<Self, String> {
        validate_record(&record)?;
        let session_secret = record.session_secret()?;
        Ok(Self {
            bearer,
            pin_hash: record.pin_hash().to_owned(),
            session_secret,
            lockout: Mutex::new(LockoutState::default()),
        })
    }

    pub fn bearer_printable(&self) -> &str {
        self.bearer.printable()
    }

    pub fn permanent_log_secrets(&self) -> Vec<String> {
        vec![
            self.bearer.printable().to_owned(),
            self.pin_hash.clone(),
            encode_hex(&self.session_secret),
        ]
    }

    pub fn authorize(&self, input: AuthInput<'_>) -> AuthDecision {
        self.authorize_at(input, unix_seconds())
    }

    fn authorize_at(&self, input: AuthInput<'_>, now_unix: u64) -> AuthDecision {
        if let Some(header) = input.authorization {
            let Some(candidate) = bearer_value(header) else {
                return AuthDecision::Unauthorized;
            };
            return if self.bearer.matches(candidate) {
                AuthDecision::Authorized(AuthSource::Bearer)
            } else {
                AuthDecision::Unauthorized
            };
        }
        if let Some(cookie_header) = input.cookie
            && let Some(candidate) = cookie_value(cookie_header, COOKIE_NAME)
            && self.session_matches(candidate, now_unix)
        {
            return AuthDecision::Authorized(AuthSource::SessionCookie(AuthSessionIdentity(
                Sha256::digest(candidate.as_bytes()).into(),
            )));
        }
        AuthDecision::Unauthorized
    }

    pub fn verify_pin(&self, candidate: &str) -> PinVerification {
        self.verify_pin_at(candidate, Instant::now())
    }

    fn verify_pin_at(&self, candidate: &str, now: Instant) -> PinVerification {
        let Ok(mut state) = self.lockout.lock() else {
            return PinVerification::Locked {
                failure_count: LOCKOUT_THRESHOLD,
                remaining: FIRST_LOCKOUT,
            };
        };
        if let Some(until) = state.locked_until {
            if now < until {
                return PinVerification::Locked {
                    failure_count: state.failure_count,
                    remaining: until.duration_since(now),
                };
            }
            state.locked_until = None;
        }

        let verified = PasswordHash::new(&self.pin_hash).ok().is_some_and(|hash| {
            production_argon2()
                .verify_password(candidate.as_bytes(), &hash)
                .is_ok()
        });
        if verified {
            *state = LockoutState::default();
            return PinVerification::Success;
        }

        state.failure_count = state.failure_count.saturating_add(1);
        let lockout = lockout_duration(state.failure_count);
        if let Some(duration) = lockout {
            state.locked_until = now.checked_add(duration);
        }
        PinVerification::Invalid {
            failure_count: state.failure_count,
            lockout,
        }
    }

    pub fn lockout_remaining(&self) -> Duration {
        self.lockout_remaining_at(Instant::now())
    }

    fn lockout_remaining_at(&self, now: Instant) -> Duration {
        let Ok(state) = self.lockout.lock() else {
            return FIRST_LOCKOUT;
        };
        state
            .locked_until
            .filter(|until| *until > now)
            .map_or(Duration::ZERO, |until| until.duration_since(now))
    }

    pub fn issue_session_cookie(&self, remember: bool, secure: bool) -> SessionCookie {
        self.issue_session_cookie_at(unix_seconds(), remember, secure)
    }

    fn issue_session_cookie_at(
        &self,
        now_unix: u64,
        remember: bool,
        secure: bool,
    ) -> SessionCookie {
        let expires = now_unix.saturating_add(SESSION_MAX_AGE_SECONDS);
        let message = format!("v1.{expires}");
        let mut mac =
            HmacSha256::new_from_slice(&self.session_secret).expect("HMAC accepts a 256-bit key");
        mac.update(message.as_bytes());
        let value = format!("{message}.{}", encode_hex(&mac.finalize().into_bytes()));
        let mut header = format!("{COOKIE_NAME}={value}; Path=/; HttpOnly; SameSite=Lax");
        if remember {
            header.push_str(&format!("; Max-Age={SESSION_MAX_AGE_SECONDS}"));
        }
        if secure {
            header.push_str("; Secure");
        }
        SessionCookie {
            header,
            sensitive_value: value,
        }
    }

    fn session_matches(&self, candidate: &str, now_unix: u64) -> bool {
        let mut parts = candidate.split('.');
        let Some(version) = parts.next() else {
            return false;
        };
        let Some(expires_text) = parts.next() else {
            return false;
        };
        let Some(mac_text) = parts.next() else {
            return false;
        };
        if parts.next().is_some() || version != "v1" {
            return false;
        }
        let Ok(expires) = expires_text.parse::<u64>() else {
            return false;
        };
        if expires < now_unix {
            return false;
        }
        let Ok(candidate_mac) = decode_hex(mac_text) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.session_secret) else {
            return false;
        };
        mac.update(format!("v1.{expires}").as_bytes());
        mac.verify_slice(&candidate_mac).is_ok()
    }
}

fn lockout_duration(failure_count: u32) -> Option<Duration> {
    if failure_count < LOCKOUT_THRESHOLD {
        return None;
    }
    let exponent = (failure_count - LOCKOUT_THRESHOLD).min(MAX_LOCKOUT_EXPONENT);
    FIRST_LOCKOUT.checked_mul(1_u32 << exponent)
}

fn bearer_value(header: &str) -> Option<&str> {
    let (scheme, value) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || value.is_empty() || value.contains(' ') {
        return None;
    }
    Some(value)
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if !value.len().is_multiple_of(2) {
        return Err(());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u8, ()> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn service(pin: &str) -> AuthService {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.json");
        mimageviewer_ipc::set_pin_file(&path, pin).unwrap();
        let record = mimageviewer_ipc::load_pin_file(&path).unwrap();
        AuthService::new(record, AuthToken::from_printable_for_test(TEST_TOKEN)).unwrap()
    }

    #[test]
    fn fifth_failure_locks_and_lockout_expires() {
        let auth = service("123456");
        let start = Instant::now();
        for expected in 1..LOCKOUT_THRESHOLD {
            assert_eq!(
                auth.verify_pin_at("654321", start),
                PinVerification::Invalid {
                    failure_count: expected,
                    lockout: None,
                }
            );
        }
        assert_eq!(
            auth.verify_pin_at("654321", start),
            PinVerification::Invalid {
                failure_count: LOCKOUT_THRESHOLD,
                lockout: Some(FIRST_LOCKOUT),
            }
        );
        assert!(matches!(
            auth.verify_pin_at("123456", start + Duration::from_secs(1)),
            PinVerification::Locked { .. }
        ));
        assert_eq!(
            auth.verify_pin_at("123456", start + FIRST_LOCKOUT),
            PinVerification::Success
        );
        assert_eq!(
            auth.lockout_remaining_at(start + FIRST_LOCKOUT),
            Duration::ZERO
        );
    }

    #[test]
    fn bearer_and_signed_session_cookie_are_accepted() {
        let auth = service("123456");
        let bearer = format!("Bearer {TEST_TOKEN}");
        assert_eq!(
            auth.authorize_at(
                AuthInput {
                    authorization: Some(&bearer),
                    cookie: None,
                },
                10
            ),
            AuthDecision::Authorized(AuthSource::Bearer)
        );
        let issued = auth.issue_session_cookie_at(10, true, false);
        assert!(issued.header.contains("Max-Age=7776000"));
        assert!(!issued.header.contains("Secure"));
        let cookie = format!("{COOKIE_NAME}={}", issued.sensitive_value);
        assert!(matches!(
            auth.authorize_at(
                AuthInput {
                    cookie: Some(&cookie),
                    ..AuthInput::default()
                },
                11
            ),
            AuthDecision::Authorized(AuthSource::SessionCookie(_))
        ));
        assert_eq!(
            auth.authorize_at(
                AuthInput {
                    cookie: Some(&cookie),
                    ..AuthInput::default()
                },
                10 + SESSION_MAX_AGE_SECONDS + 1
            ),
            AuthDecision::Unauthorized
        );
    }

    #[test]
    fn session_cookie_respects_remember_and_secure_flags() {
        let auth = service("123456");
        let session_only = auth.issue_session_cookie_at(10, false, true);
        assert!(!session_only.header.contains("Max-Age"));
        assert!(session_only.header.contains("; Secure"));
    }

    #[test]
    fn malformed_authorization_does_not_fall_back_to_cookie() {
        let auth = service("123456");
        let issued = auth.issue_session_cookie_at(10, true, false);
        let cookie = format!("{COOKIE_NAME}={}", issued.sensitive_value);
        assert_eq!(
            auth.authorize_at(
                AuthInput {
                    authorization: Some("Basic abc"),
                    cookie: Some(&cookie),
                },
                11
            ),
            AuthDecision::Unauthorized
        );
    }

    #[test]
    fn generated_bearer_has_256_bits_of_random_input_as_hex() {
        let token = AuthToken::generate().expect("OS random source");
        assert_eq!(token.printable().len(), TOKEN_HEX_LEN);
        assert!(
            token
                .printable()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }
}
