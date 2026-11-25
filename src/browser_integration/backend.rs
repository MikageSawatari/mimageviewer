//! Worker-only native registration. Tests inject environment, registry and file effects.
use super::{Action, Completion, Prepared, Selection};
use mimageviewer_browser_host::{
    Config,
    protocol::{NATIVE_HOST_NAME, ORIGINS},
};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const HOST_EXE: &str = "mimageviewer-browser-host.exe";
pub const REGISTRY_KEYS: [&str; 2] = [
    r"Software\Google\Chrome\NativeMessagingHosts\to.mikage.mimageviewer",
    r"Software\Microsoft\Edge\NativeMessagingHosts\to.mikage.mimageviewer",
];

#[derive(Clone, Debug)]
pub struct Environment {
    pub app_data: PathBuf,
    pub data_dir: PathBuf,
    pub launcher: Option<PathBuf>,
    pub host_source: PathBuf,
    pub default_output: PathBuf,
    pub portable: bool,
    pub explicit_data_dir: bool,
}

impl Environment {
    pub fn capture() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let explicit_data_dir = std::env::args_os()
            .skip(1)
            .take_while(|arg| arg != "--")
            .any(|arg| arg == "--data-dir");
        Ok(Self {
            app_data: std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or("APPDATA を確認できません")?,
            data_dir: crate::data_dir::get(),
            launcher: std::env::var_os("MIV_LAUNCHER_EXE_PATH").map(PathBuf::from),
            host_source: executable
                .parent()
                .ok_or("実行ファイルを確認できません")?
                .join(HOST_EXE),
            default_output: crate::browser_view_policy::output_root_from_pictures(
                &crate::known_folders::pictures_dir().ok_or("ピクチャを確認できません")?,
            ),
            portable: cfg!(feature = "portable"),
            explicit_data_dir,
        })
    }
    pub fn managed(&self) -> PathBuf {
        // Component joins keep one separator style: the uninstaller compares the
        // registry value with `{userappdata}\mimageviewer\browser-host\<name>.json`.
        self.app_data.join("mimageviewer").join("browser-host")
    }
    pub fn manifest(&self) -> PathBuf {
        self.managed().join(format!("{NATIVE_HOST_NAME}.json"))
    }
    pub fn eligibility(&self) -> Result<(), String> {
        if self.portable {
            return Err("ポータブル版ではブラウザ連携を登録できません。".into());
        }
        if self.explicit_data_dir {
            return Err("--data-dir を指定した起動では登録を変更できません。".into());
        }
        if !self.app_data.is_absolute()
            || !self.data_dir.is_absolute()
            || !self.host_source.is_absolute()
            || !self.default_output.is_absolute()
        {
            return Err("連携に使うフォルダの絶対パスを確認できません。".into());
        }
        let default = self.app_data.join("mimageviewer");
        let current = fs::canonicalize(&self.data_dir)
            .or_else(|_| std::path::absolute(&self.data_dir))
            .map_err(|e| e.to_string())?;
        let default = fs::canonicalize(&default)
            .or_else(|_| std::path::absolute(&default))
            .map_err(|e| e.to_string())?;
        if !current
            .as_os_str()
            .eq_ignore_ascii_case(default.as_os_str())
        {
            return Err("既定のデータフォルダの mIV だけが登録を変更できます。".into());
        }
        if !self
            .launcher
            .as_ref()
            .is_some_and(|path| path.is_absolute() && path.is_file())
        {
            return Err(
                "mIV の起動元を確認できません。mimageviewer.exe から起動してください。".into(),
            );
        }
        if !self.host_source.is_file() {
            return Err("連携用の exe がありません。mIV を入れ直してください。".into());
        }
        Ok(())
    }
}

pub trait Registry {
    fn get(&mut self, key: &str) -> Result<Option<PathBuf>, String>;
    fn set(&mut self, key: &str, value: &Path) -> Result<(), String>;
    fn remove(&mut self, key: &str) -> Result<(), String>;
}
pub struct NativeRegistry;
#[cfg(windows)]
impl Registry for NativeRegistry {
    fn get(&mut self, key: &str) -> Result<Option<PathBuf>, String> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(key)
            .and_then(|key| key.get_value::<String, _>(""))
        {
            Ok(value) => Ok(Some(PathBuf::from(value))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    fn set(&mut self, key: &str, value: &Path) -> Result<(), String> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(key)
            .map_err(|e| e.to_string())?;
        key.set_value("", &value.to_str().ok_or("パスを登録できません")?)
            .map_err(|e| e.to_string())
    }
    fn remove(&mut self, key: &str) -> Result<(), String> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(key) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
#[cfg(not(windows))]
impl Registry for NativeRegistry {
    fn get(&mut self, _: &str) -> Result<Option<PathBuf>, String> {
        Err("Windows で利用できます。".into())
    }
    fn set(&mut self, _: &str, _: &Path) -> Result<(), String> {
        Err("Windows で利用できます。".into())
    }
    fn remove(&mut self, _: &str) -> Result<(), String> {
        Err("Windows で利用できます。".into())
    }
}

pub trait Files {
    fn copy_host(&mut self, source: &Path, target: &Path) -> io::Result<()>;
    fn write_config(&mut self, target: &Path, bytes: &[u8]) -> io::Result<()>;
    fn remove_exe(&mut self, target: &Path) -> io::Result<()>;
}
pub struct NativeFiles;
impl Files for NativeFiles {
    fn copy_host(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        fs::copy(source, target).map(|_| ())
    }
    fn write_config(&mut self, target: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(target, bytes)
    }
    fn remove_exe(&mut self, target: &Path) -> io::Result<()> {
        fs::remove_file(target)
    }
}

fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn plain_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || reparse(&metadata) {
        return Err("登録先のフォルダがリンクになっています。".into());
    }
    Ok(())
}
/// A directory junction or symlink reports `is_dir() == false` through
/// `symlink_metadata`, so the directory attribute decides how to unlink it.
fn directory_attribute(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x10 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_dir()
    }
}
fn remove_tree_no_follow(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if reparse(&metadata) {
        return if directory_attribute(&metadata) {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            remove_tree_no_follow(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn validate_output(env: &Environment, output: &Path) -> Result<(), String> {
    if !output.is_absolute() {
        return Err("保存先は絶対パスで指定してください。".into());
    }
    if mimageviewer_capture_file::is_browser_host_destination(output, &env.managed())? {
        return Err(
            "ブラウザ連携の管理フォルダには画像を保存できません。別の保存先を選んでください。"
                .into(),
        );
    }
    Ok(())
}

fn current_generation(env: &Environment, output: &Path) -> Option<PathBuf> {
    if reparse(&fs::symlink_metadata(env.manifest()).ok()?) {
        return None;
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(env.manifest()).ok()?).ok()?;
    if manifest["name"] != NATIVE_HOST_NAME
        || manifest["type"] != "stdio"
        || manifest["allowed_origins"] != serde_json::json!(ORIGINS)
    {
        return None;
    }
    let host = PathBuf::from(manifest["path"].as_str()?);
    let generation = host.parent()?;
    if generation.parent()? != env.managed() || host.file_name()? != HOST_EXE {
        return None;
    }
    if !generation
        .file_name()?
        .to_string_lossy()
        .starts_with("generation-")
    {
        return None;
    }
    plain_directory(generation).ok()?;
    if reparse(&fs::symlink_metadata(&host).ok()?) {
        return None;
    }
    if reparse(&fs::symlink_metadata(generation.join("config.json")).ok()?) {
        return None;
    }
    let config: Config =
        serde_json::from_slice(&fs::read(generation.join("config.json")).ok()?).ok()?;
    if config.v != 1 || config.launcher != *env.launcher.as_ref()? || config.output_dir != output {
        return None;
    }
    // These are small companion executables; all reads/hashes stay on the worker.
    use sha2::Digest;
    if sha2::Sha256::digest(fs::read(&host).ok()?)
        != sha2::Sha256::digest(fs::read(&env.host_source).ok()?)
    {
        return None;
    }
    Some(generation.to_owned())
}

pub fn cleanup(
    env: &Environment,
    keep: Option<&Path>,
    files: &mut impl Files,
) -> Result<(), String> {
    if !env.managed().exists() {
        return Ok(());
    }
    plain_directory(&env.managed())?;
    for entry in fs::read_dir(env.managed()).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if Some(path.as_path()) == keep
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with("generation-")
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || reparse(&metadata) {
            continue;
        }
        let exe = path.join(HOST_EXE);
        if fs::symlink_metadata(&exe).is_ok_and(|metadata| reparse(&metadata)) {
            continue;
        }
        // Do not delete config.json while its host may still be reading it.
        match files.remove_exe(&exe) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                crate::logger::log(format!("browser_registration: keeping generation: {error}"));
                continue;
            }
        }
        remove_tree_no_follow(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn register(
    env: &Environment,
    output: &Path,
    registry: &mut impl Registry,
    files: &mut impl Files,
) -> Result<(), String> {
    env.eligibility()?;
    validate_output(env, output)?;
    fs::create_dir_all(env.managed()).map_err(|e| e.to_string())?;
    plain_directory(&env.managed())?;
    let generation = if let Some(current) = current_generation(env, output) {
        current
    } else {
        let generation = tempfile::Builder::new()
            .prefix("generation-")
            .tempdir_in(env.managed())
            .map_err(|e| e.to_string())?;
        files
            .copy_host(&env.host_source, &generation.path().join(HOST_EXE))
            .map_err(|e| e.to_string())?;
        let config = Config {
            v: 1,
            launcher: env.launcher.clone().ok_or("launcher が分かりません")?,
            output_dir: output.to_owned(),
        };
        files
            .write_config(
                &generation.path().join("config.json"),
                &serde_json::to_vec(&config).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        // Publish the completed generation before atomically replacing its pointer.
        let path = generation.keep();
        let manifest = serde_json::json!({ "name": NATIVE_HOST_NAME, "description": "mImageViewer browser integration", "path": path.join(HOST_EXE), "type": "stdio", "allowed_origins": ORIGINS });
        let mut temporary =
            tempfile::NamedTempFile::new_in(env.managed()).map_err(|e| e.to_string())?;
        temporary
            .write_all(&serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)
            .and_then(|()| temporary.flush())
            .map_err(|e| e.to_string())?;
        temporary
            .persist(env.manifest())
            .map_err(|e| e.to_string())?;
        path
    };
    for key in REGISTRY_KEYS {
        if registry.get(key)?.as_ref() != Some(&env.manifest()) {
            registry.set(key, &env.manifest())?;
        }
    }
    cleanup(env, Some(&generation), files)
}

pub fn unregister(
    env: &Environment,
    registry: &mut impl Registry,
    files: &mut impl Files,
) -> Result<(), String> {
    env.eligibility()?;
    for key in REGISTRY_KEYS {
        if registry.get(key)?.as_ref().is_some_and(|path| {
            path.as_os_str()
                .eq_ignore_ascii_case(env.manifest().as_os_str())
        }) {
            registry.remove(key)?;
        }
    }
    if env.managed().exists() {
        plain_directory(&env.managed())?;
        match fs::remove_file(env.manifest()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        cleanup(env, None, files)?;
        let _ = fs::remove_dir(env.managed()); // A running generation may remain.
    }
    Ok(())
}

pub struct NativeBackend;
pub trait Folders {
    fn choose(&mut self, current: &Path) -> Result<Option<PathBuf>, String>;
    fn open(&mut self, path: &Path) -> Result<(), String>;
}
struct NativeFolders;
impl Folders for NativeFolders {
    fn choose(&mut self, current: &Path) -> Result<Option<PathBuf>, String> {
        Ok(rfd::FileDialog::new().set_directory(current).pick_folder())
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        #[cfg(windows)]
        {
            std::process::Command::new("explorer.exe")
                .arg(path)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        #[cfg(not(windows))]
        let _ = path;
        Ok(())
    }
}
pub fn prepare_with(
    env: Environment,
    selection: Selection,
    registry: &mut impl Registry,
    files: &mut impl Files,
) -> super::Preparation {
    let default_output = Some(env.default_output.clone());
    let ready = (|| {
        env.eligibility()?;
        let mut message = None;
        if selection.enabled {
            let output = selection.output_dir.as_ref().unwrap_or(&env.default_output);
            if let Err(error) = register(&env, output, registry, files) {
                message = Some(format!("登録の更新に失敗しました: {error}"));
            }
        }
        let values = REGISTRY_KEYS
            .iter()
            .map(|key| registry.get(key))
            .collect::<Result<Vec<_>, _>>()?;
        let registered = values.iter().all(|value| *value == Some(env.manifest()));
        Ok(Prepared {
            env,
            selection,
            registered,
            message,
        })
    })();
    super::Preparation {
        default_output,
        ready,
    }
}
impl super::Backend for NativeBackend {
    fn persist(&self, selection: &Selection) -> Result<(), String> {
        crate::settings::save_browser_integration_worker(selection.enabled, &selection.output_dir)
    }
    fn prepare(&self, selection: Selection) -> super::Preparation {
        match Environment::capture() {
            Ok(env) => prepare_with(env, selection, &mut NativeRegistry, &mut NativeFiles),
            Err(reason) => super::Preparation {
                default_output: None,
                ready: Err(reason),
            },
        }
    }
    fn perform(&self, prepared: Prepared, action: Action) -> Result<Completion, String> {
        perform_with(
            prepared,
            action,
            &mut NativeRegistry,
            &mut NativeFiles,
            &mut NativeFolders,
        )
    }
}

pub fn perform_with(
    mut prepared: Prepared,
    action: Action,
    registry: &mut impl Registry,
    files: &mut impl Files,
    folders: &mut impl Folders,
) -> Result<Completion, String> {
    prepared.env.eligibility()?;
    let removing = matches!(&action, Action::Remove);
    let selection = match action {
        Action::Apply(selection) => Some(selection),
        Action::Remove => Some(Selection {
            enabled: false,
            ..prepared.selection.clone()
        }),
        Action::ChooseFolder => {
            let selected = folders.choose(prepared.effective_output())?;
            selected.map(|output| Selection {
                output_dir: Some(output),
                ..prepared.selection.clone()
            })
        }
        Action::OpenFolder(output) => {
            let path = output.as_deref().unwrap_or(&prepared.env.default_output);
            validate_output(&prepared.env, path)?;
            if !path.is_dir() {
                return Err(crate::known_folders::CAPTURE_DESTINATION_NOT_FOUND.into());
            }
            folders.open(path)?;
            None
        }
    };
    if let Some(selection) = &selection {
        let output = selection
            .output_dir
            .as_ref()
            .unwrap_or(&prepared.env.default_output);
        if selection.enabled {
            register(&prepared.env, output, registry, files)?;
        } else if removing {
            unregister(&prepared.env, registry, files)?;
        } else {
            validate_output(&prepared.env, output)?;
            if prepared.selection.enabled {
                unregister(&prepared.env, registry, files)?;
            }
        }
        prepared.selection = selection.clone();
        prepared.registered = selection.enabled;
    }
    prepared.message = Some(
        if selection.is_some() {
            "ブラウザ連携の設定を反映しました。"
        } else {
            "フォルダ操作を完了しました。"
        }
        .into(),
    );
    Ok(Completion {
        prepared,
        selection,
    })
}
