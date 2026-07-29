use subtle::ConstantTimeEq;

pub const COOKIE_NAME: &str = "miv_remote_token";
const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;

#[derive(Clone)]
pub struct AuthToken {
    printable: String,
    expected_ascii: [u8; TOKEN_HEX_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Bearer,
    Cookie,
    Query,
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
    pub query_token: Option<&'a str>,
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
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut expected_ascii = [0_u8; TOKEN_HEX_LEN];
        for (idx, byte) in raw.iter().copied().enumerate() {
            expected_ascii[idx * 2] = HEX[(byte >> 4) as usize];
            expected_ascii[idx * 2 + 1] = HEX[(byte & 0x0f) as usize];
        }
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

    pub fn authorize(&self, input: AuthInput<'_>) -> AuthDecision {
        if let Some(header) = input.authorization {
            let Some(candidate) = bearer_value(header) else {
                return AuthDecision::Unauthorized;
            };
            return self.decision(candidate, AuthSource::Bearer);
        }

        if let Some(candidate) = input.query_token {
            return self.decision(candidate, AuthSource::Query);
        }

        if let Some(cookie_header) = input.cookie
            && let Some(candidate) = cookie_value(cookie_header, COOKIE_NAME)
        {
            return self.decision(candidate, AuthSource::Cookie);
        }

        AuthDecision::Unauthorized
    }

    fn decision(&self, candidate: &str, source: AuthSource) -> AuthDecision {
        if self.matches(candidate) {
            AuthDecision::Authorized(source)
        } else {
            AuthDecision::Unauthorized
        }
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

impl AuthDecision {
    pub fn http_status(self) -> u16 {
        match self {
            Self::Authorized(_) => 200,
            Self::Unauthorized => 401,
        }
    }
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

pub fn session_cookie(token: &AuthToken) -> String {
    format!(
        "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Lax",
        token.printable()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> AuthToken {
        AuthToken::from_printable_for_test(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
    }

    #[test]
    fn missing_token_is_unauthorized() {
        assert_eq!(token().authorize(AuthInput::default()).http_status(), 401);
    }

    #[test]
    fn mismatched_token_is_unauthorized() {
        assert_eq!(
            token()
                .authorize(AuthInput {
                    authorization: Some(
                        "Bearer ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                    ),
                    ..AuthInput::default()
                })
                .http_status(),
            401
        );
    }

    #[test]
    fn different_length_is_unauthorized() {
        let long = format!("{}00", token().printable());
        for candidate in ["", "0123456789abcdef", long.as_str()] {
            let authorization = format!("Bearer {candidate}");
            assert_eq!(
                token()
                    .authorize(AuthInput {
                        authorization: Some(&authorization),
                        ..AuthInput::default()
                    })
                    .http_status(),
                401
            );
        }
    }

    #[test]
    fn bearer_cookie_and_query_are_accepted() {
        let token = token();
        let bearer = format!("Bearer {}", token.printable());
        let cookie = format!("theme=dark; {COOKIE_NAME}={}", token.printable());

        assert_eq!(
            token.authorize(AuthInput {
                authorization: Some(&bearer),
                ..AuthInput::default()
            }),
            AuthDecision::Authorized(AuthSource::Bearer)
        );
        assert_eq!(
            token.authorize(AuthInput {
                cookie: Some(&cookie),
                ..AuthInput::default()
            }),
            AuthDecision::Authorized(AuthSource::Cookie)
        );
        assert_eq!(
            token.authorize(AuthInput {
                query_token: Some(token.printable()),
                ..AuthInput::default()
            }),
            AuthDecision::Authorized(AuthSource::Query)
        );
    }

    #[test]
    fn malformed_authorization_does_not_fall_back_to_cookie() {
        let token = token();
        let cookie = format!("{COOKIE_NAME}={}", token.printable());
        assert_eq!(
            token.authorize(AuthInput {
                authorization: Some("Basic abc"),
                cookie: Some(&cookie),
                query_token: Some(token.printable()),
            }),
            AuthDecision::Unauthorized
        );
    }

    #[test]
    fn query_token_replaces_a_stale_session_cookie() {
        let token = token();
        let stale_cookie = format!("{COOKIE_NAME}={}", "f".repeat(TOKEN_HEX_LEN));
        assert_eq!(
            token.authorize(AuthInput {
                cookie: Some(&stale_cookie),
                query_token: Some(token.printable()),
                ..AuthInput::default()
            }),
            AuthDecision::Authorized(AuthSource::Query)
        );
    }

    #[test]
    fn generated_token_has_256_bits_of_random_input_as_hex() {
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
