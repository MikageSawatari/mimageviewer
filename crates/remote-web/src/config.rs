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
    pub public_url: Option<String>,
    pub web_root: PathBuf,
    /// core が所有する子プロセス。IPC を失ったままなら孤児化を避けるため終了する。
    pub managed_by_core: bool,
}

impl Config {
    pub fn parse() -> Result<Self, String> {
        Self::parse_args(std::env::args_os().skip(1))
    }

    fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut bind = IpAddr::from([127, 0, 0, 1]);
        let mut port = DEFAULT_PORT;
        let mut data_dir: Option<PathBuf> = None;
        let mut log_path = None;
        let mut auth_path = None;
        let mut public_url = None;
        let mut managed_by_core = false;
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
                    log_path = Some(PathBuf::from(
                        args.next().ok_or("--log にはファイルパスが必要です")?,
                    ));
                }
                "--auth-file" => {
                    auth_path = Some(PathBuf::from(
                        args.next()
                            .ok_or("--auth-file にはファイルパスが必要です")?,
                    ));
                }
                "--url" => {
                    let value = args.next().ok_or("--url には接続 URL が必要です")?;
                    public_url = Some(value.to_string_lossy().into_owned());
                }
                "--managed-by-core" => managed_by_core = true,
                "--help" | "-h" => return Err(help_text().to_owned()),
                other => return Err(format!("不明な引数です: {other}\n\n{}", help_text())),
            }
        }

        let data_dir = data_dir.unwrap_or_else(default_data_dir);
        let log_path =
            log_path.ok_or("--log は必須です。本体が決めた診断ログのパスを指定してください")?;
        let auth_path = auth_path
            .ok_or("--auth-file は必須です。本体が所有する認証ファイルのパスを指定してください")?;
        #[cfg(feature = "embedded-web-assets")]
        let web_root = PathBuf::new();
        #[cfg(not(feature = "embedded-web-assets"))]
        let web_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
        Ok(Self {
            bind,
            port,
            data_dir,
            log_path,
            auth_path,
            public_url,
            web_root,
            managed_by_core,
        })
    }
}

pub fn default_data_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(appdata).join("mimageviewer")
}

fn help_text() -> &'static str {
    "mimageviewer-remote [--bind <IP>] [--port <PORT>] [--data-dir <DIR>] \
     --log <FILE> --auth-file <FILE> [--url <BASE_URL>]\n\
     既定: --bind 127.0.0.1 --port 8787 --data-dir %APPDATA%\\mimageviewer\n\
     PIN は mImageViewer 本体の「リモート接続」ダイアログで設定します。"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_log_and_public_url_options() {
        let config = Config::parse_args([
            OsString::from("--auth-file"),
            OsString::from("auth.json"),
            OsString::from("--log"),
            OsString::from("remote.log"),
            OsString::from("--url"),
            OsString::from("https://example.ts.net/"),
        ])
        .unwrap();
        assert_eq!(config.auth_path, PathBuf::from("auth.json"));
        assert_eq!(config.log_path, PathBuf::from("remote.log"));
        assert_eq!(
            config.public_url.as_deref(),
            Some("https://example.ts.net/")
        );
    }

    #[test]
    fn managed_marker_is_typed_and_external_launch_stays_unmanaged() {
        let base = [
            OsString::from("--auth-file"),
            OsString::from("auth.json"),
            OsString::from("--log"),
            OsString::from("remote.log"),
        ];
        assert!(!Config::parse_args(base.clone()).unwrap().managed_by_core);
        assert!(
            Config::parse_args(
                base.into_iter()
                    .chain([OsString::from("--managed-by-core")])
            )
            .unwrap()
            .managed_by_core
        );
    }

    #[test]
    fn set_pin_option_is_removed() {
        let error = Config::parse_args([
            OsString::from("--auth-file"),
            OsString::from("auth.json"),
            OsString::from("--log"),
            OsString::from("remote.log"),
            OsString::from("--set-pin"),
            OsString::from("123456"),
        ])
        .unwrap_err();
        assert!(error.contains("不明な引数です: --set-pin"));
    }

    #[cfg(feature = "embedded-web-assets")]
    #[test]
    fn distribution_has_no_source_tree_web_root_dependency() {
        let config = Config::parse_args([
            OsString::from("--auth-file"),
            OsString::from("auth.json"),
            OsString::from("--log"),
            OsString::from("remote.log"),
        ])
        .unwrap();
        assert!(config.web_root.as_os_str().is_empty());
    }
}
