use std::ffi::OsString;
use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 8787;

#[derive(Debug)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub data_dir: PathBuf,
    pub log_path: PathBuf,
    pub auth_path: PathBuf,
    pub thumb_cache_path: PathBuf,
    pub set_pin: Option<String>,
    pub public_url: Option<String>,
    pub web_root: PathBuf,
}

impl Config {
    pub fn parse() -> Result<Self, String> {
        Self::parse_args(std::env::args_os().skip(1))
    }

    fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut bind = IpAddr::from([127, 0, 0, 1]);
        let mut port = DEFAULT_PORT;
        let mut data_dir: Option<PathBuf> = None;
        let mut log_path = PathBuf::from("remote-web-log.jsonl");
        let mut auth_path = PathBuf::from("remote-web-auth.json");
        let mut thumb_cache_path = PathBuf::from("remote-web-thumbs.db");
        let mut set_pin = None;
        let mut public_url = None;
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            match arg.to_string_lossy().as_ref() {
                "--bind" => {
                    let value = args.next().ok_or("--bind には IP アドレスが必要です")?;
                    bind = value
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--bind は IP アドレスで指定してください")?;
                }
                "--port" => {
                    let value = args.next().ok_or("--port にはポート番号が必要です")?;
                    port = value
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--port は 1..65535 で指定してください")?;
                    if port == 0 {
                        return Err("--port は 1..65535 で指定してください".to_owned());
                    }
                }
                "--data-dir" => {
                    data_dir = Some(PathBuf::from(
                        args.next().ok_or("--data-dir にはディレクトリが必要です")?,
                    ));
                }
                "--log" => {
                    log_path =
                        PathBuf::from(args.next().ok_or("--log にはファイルパスが必要です")?);
                }
                "--auth-file" => {
                    auth_path = PathBuf::from(
                        args.next()
                            .ok_or("--auth-file にはファイルパスが必要です")?,
                    );
                }
                "--thumb-cache" => {
                    thumb_cache_path = PathBuf::from(
                        args.next()
                            .ok_or("--thumb-cache にはファイルパスが必要です")?,
                    );
                }
                "--set-pin" => {
                    let value = args.next().ok_or("--set-pin には PIN が必要です")?;
                    set_pin = Some(value.to_string_lossy().into_owned());
                }
                "--url" => {
                    let value = args.next().ok_or("--url には接続 URL が必要です")?;
                    public_url = Some(value.to_string_lossy().into_owned());
                }
                "--help" | "-h" => return Err(help_text().to_owned()),
                other => return Err(format!("不明な引数です: {other}\n\n{}", help_text())),
            }
        }

        let data_dir = data_dir.unwrap_or_else(default_data_dir);
        let web_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
        Ok(Self {
            bind,
            port,
            data_dir,
            log_path,
            auth_path,
            thumb_cache_path,
            set_pin,
            public_url,
            web_root,
        })
    }
}

pub fn default_data_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(appdata).join("mimageviewer")
}

fn help_text() -> &'static str {
    "mimageviewer-remote [--bind <IP>] [--port <PORT>] [--data-dir <DIR>] [--log <FILE>]\n\
     [--auth-file <FILE>] [--thumb-cache <FILE>] [--set-pin <PIN>] [--url <BASE_URL>]\n\
     既定: --bind 127.0.0.1 --port 8787 --data-dir %APPDATA%\\mimageviewer \
     --log .\\remote-web-log.jsonl --auth-file .\\remote-web-auth.json \
     --thumb-cache .\\remote-web-thumbs.db\n\
     初回設定: mimageviewer-remote --set-pin <6文字以上のPIN>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pin_auth_and_public_url_options() {
        let config = Config::parse_args([
            OsString::from("--auth-file"),
            OsString::from("auth.json"),
            OsString::from("--set-pin"),
            OsString::from("123456"),
            OsString::from("--thumb-cache"),
            OsString::from("thumbs.db"),
            OsString::from("--url"),
            OsString::from("https://example.ts.net/"),
        ])
        .unwrap();
        assert_eq!(config.auth_path, PathBuf::from("auth.json"));
        assert_eq!(config.set_pin.as_deref(), Some("123456"));
        assert_eq!(config.thumb_cache_path, PathBuf::from("thumbs.db"));
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://example.ts.net/")
        );
    }
}
