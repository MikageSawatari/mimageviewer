use std::net::IpAddr;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 8787;

#[derive(Debug)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub data_dir: PathBuf,
    pub web_root: PathBuf,
}

impl Config {
    pub fn parse() -> Result<Self, String> {
        let mut bind = IpAddr::from([127, 0, 0, 1]);
        let mut port = DEFAULT_PORT;
        let mut data_dir: Option<PathBuf> = None;
        let mut args = std::env::args_os().skip(1);

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
            web_root,
        })
    }
}

fn default_data_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(appdata).join("mimageviewer")
}

fn help_text() -> &'static str {
    "mimageviewer-remote [--bind <IP>] [--port <PORT>] [--data-dir <DIR>]\n\
     既定: --bind 127.0.0.1 --port 8787 --data-dir %APPDATA%\\mimageviewer"
}
