use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};

pub const AUTH_FILE_VERSION: u32 = 1;
pub const MIN_PIN_CHARS: usize = 6;
pub const MAX_PIN_CHARS: usize = 1024;
const SESSION_SECRET_BYTES: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthRecord {
    version: u32,
    pin_hash: String,
    session_secret_hex: String,
}

impl AuthRecord {
    pub fn pin_hash(&self) -> &str {
        &self.pin_hash
    }

    pub fn session_secret(&self) -> Result<[u8; SESSION_SECRET_BYTES], String> {
        decode_secret(&self.session_secret_hex)
    }
}

pub fn validate_pin(pin: &str) -> Result<(), String> {
    let length = pin.chars().count();
    if length < MIN_PIN_CHARS {
        return Err(format!("PIN は {MIN_PIN_CHARS} 文字以上にしてください"));
    }
    if length > MAX_PIN_CHARS {
        return Err(format!("PIN は {MAX_PIN_CHARS} 文字以下にしてください"));
    }
    if !pin.chars().all(|character| matches!(character, '!'..='~')) {
        return Err(
            "PIN は空白を含まない印字可能な半角英数字・記号だけを使用してください".to_owned(),
        );
    }
    Ok(())
}

pub fn production_argon2() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
}

pub fn set_pin_file(path: &Path, pin: &str) -> Result<(), String> {
    validate_pin(pin)?;

    let mut salt_bytes = [0_u8; 16];
    let mut session_secret = [0_u8; SESSION_SECRET_BYTES];
    getrandom::fill(&mut salt_bytes)
        .map_err(|error| format!("PIN 用 salt を生成できません: {error}"))?;
    getrandom::fill(&mut session_secret)
        .map_err(|error| format!("セッション秘密値を生成できません: {error}"))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| format!("PIN 用 salt を構成できません: {error}"))?;
    let pin_hash = production_argon2()
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|error| format!("PIN をハッシュ化できません: {error}"))?
        .to_string();
    let record = AuthRecord {
        version: AUTH_FILE_VERSION,
        pin_hash,
        session_secret_hex: encode_hex(&session_secret),
    };
    write_record_atomically(path, &record)
}

pub fn rotate_session_secret_file(path: &Path) -> Result<(), String> {
    let mut record = load_pin_file(path)?;
    let mut session_secret = [0_u8; SESSION_SECRET_BYTES];
    getrandom::fill(&mut session_secret)
        .map_err(|error| format!("セッション秘密値を生成できません: {error}"))?;
    record.session_secret_hex = encode_hex(&session_secret);
    write_record_atomically(path, &record)
}

pub fn load_pin_file(path: &Path) -> Result<AuthRecord, String> {
    if !path.is_file() {
        return Err(format!(
            "PIN が未設定です (認証ファイル: {})",
            path.display()
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("認証ファイルを読み込めません ({}): {error}", path.display()))?;
    let record: AuthRecord = serde_json::from_slice(&bytes)
        .map_err(|error| format!("認証ファイルが不正です ({}): {error}", path.display()))?;
    validate_record(&record)?;
    Ok(record)
}

pub fn validate_record(record: &AuthRecord) -> Result<(), String> {
    if record.version != AUTH_FILE_VERSION {
        return Err(format!(
            "未対応の認証ファイル version です: {}",
            record.version
        ));
    }
    let hash = PasswordHash::new(&record.pin_hash)
        .map_err(|_| "認証ファイルの PIN hash が不正です".to_owned())?;
    if hash.algorithm.as_str() != "argon2id" {
        return Err("認証ファイルの PIN hash は Argon2id ではありません".to_owned());
    }
    decode_secret(&record.session_secret_hex)?;
    Ok(())
}

fn write_record_atomically(path: &Path, record: &AuthRecord) -> Result<(), String> {
    let temporary_path = temporary_auth_path(path);
    let result = (|| {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|error| {
                format!(
                    "認証ファイルの一時ファイルを書き込めません ({}): {error}",
                    temporary_path.display()
                )
            })?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, record)
            .map_err(|error| format!("認証ファイルを JSON 化できません: {error}"))?;
        writer
            .write_all(b"\n")
            .and_then(|()| writer.flush())
            .map_err(|error| {
                format!(
                    "認証ファイルの一時ファイルを保存できません ({}): {error}",
                    temporary_path.display()
                )
            })?;
        writer.get_ref().sync_all().map_err(|error| {
            format!(
                "認証ファイルの一時ファイルを同期できません ({}): {error}",
                temporary_path.display()
            )
        })?;
        drop(writer);
        std::fs::rename(&temporary_path, path).map_err(|error| {
            format!(
                "認証ファイルを置き換えられません ({}): {error}",
                path.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn temporary_auth_path(path: &Path) -> PathBuf {
    let mut temporary_path = path.to_path_buf();
    let mut filename = temporary_path
        .file_name()
        .map(|name| name.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("remote-web-auth.json"));
    filename.push(".tmp");
    temporary_path.set_file_name(filename);
    temporary_path
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

fn decode_secret(value: &str) -> Result<[u8; SESSION_SECRET_BYTES], String> {
    let bytes = decode_hex(value).map_err(|_| "認証ファイルの session secret が不正です")?;
    bytes
        .try_into()
        .map_err(|_| "認証ファイルの session secret 長が不正です".to_owned())
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
    use argon2::PasswordVerifier;

    #[test]
    fn written_auth_file_round_trips_without_plaintext_pin() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote-web-auth.json");
        let pin = "846291-long-passphrase";

        set_pin_file(&path, pin).unwrap();
        let record = load_pin_file(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("$argon2id$"));
        assert!(!contents.contains(pin));
        let hash = PasswordHash::new(record.pin_hash()).unwrap();
        assert!(
            production_argon2()
                .verify_password(pin.as_bytes(), &hash)
                .is_ok()
        );
        assert_eq!(record.session_secret().unwrap().len(), SESSION_SECRET_BYTES);
    }

    #[test]
    fn mismatched_auth_file_version_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote-web-auth.json");
        set_pin_file(&path, "123456").unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["version"] = serde_json::json!(AUTH_FILE_VERSION + 1);
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

        let error = load_pin_file(&path).unwrap_err();
        assert!(error.contains("未対応の認証ファイル version"));
    }

    #[test]
    fn pin_length_bounds_are_shared() {
        assert!(validate_pin(&"x".repeat(MIN_PIN_CHARS - 1)).is_err());
        assert!(validate_pin(&"x".repeat(MIN_PIN_CHARS)).is_ok());
        assert!(validate_pin(&"x".repeat(MAX_PIN_CHARS)).is_ok());
        assert!(validate_pin(&"x".repeat(MAX_PIN_CHARS + 1)).is_err());
    }

    #[test]
    fn pin_accepts_only_printable_ascii_without_spaces() {
        let every_printable_ascii: String = (b'!'..=b'~').map(char::from).collect();
        assert!(validate_pin(&every_printable_ascii).is_ok());

        for rejected in [
            "日本語の秘密",
            "１２３４５６",
            "123 456",
            "12345\t6",
            "12345\n6",
            "12345\u{7f}",
        ] {
            assert!(validate_pin(rejected).is_err(), "accepted {rejected:?}");
        }
    }

    #[test]
    fn rejected_pin_is_not_included_in_the_validation_error() {
        let rejected = "秘密の番号です";
        let error = validate_pin(rejected).unwrap_err();
        assert!(!error.contains(rejected));
        assert!(error.contains("空白を含まない印字可能な半角英数字・記号"));
    }

    #[test]
    fn auth_file_write_replaces_through_a_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote-web-auth.json");
        let temporary_path = temporary_auth_path(&path);
        set_pin_file(&path, "123456").unwrap();
        let first = std::fs::read(&path).unwrap();

        std::fs::create_dir(&temporary_path).unwrap();
        assert!(set_pin_file(&path, "654321").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), first);
        std::fs::remove_dir(&temporary_path).unwrap();

        set_pin_file(&path, "654321").unwrap();

        assert_ne!(std::fs::read(&path).unwrap(), first);
        assert!(!temporary_path.exists());
        load_pin_file(&path).unwrap();
    }

    #[test]
    fn rotating_sessions_preserves_the_pin_hash_and_replaces_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote-web-auth.json");
        set_pin_file(&path, "123456").unwrap();
        let before = load_pin_file(&path).unwrap();
        let before_secret = before.session_secret().unwrap();

        rotate_session_secret_file(&path).unwrap();

        let after = load_pin_file(&path).unwrap();
        assert_eq!(after.pin_hash(), before.pin_hash());
        assert_ne!(after.session_secret().unwrap(), before_secret);
        assert!(!temporary_auth_path(&path).exists());

        let temporary_path = temporary_auth_path(&path);
        std::fs::create_dir(&temporary_path).unwrap();
        let unchanged = std::fs::read(&path).unwrap();
        assert!(rotate_session_secret_file(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), unchanged);
    }
}
