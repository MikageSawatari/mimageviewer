//! AI application registration. Every entry doing filesystem/process I/O is worker-only.
//! Environment and process snapshots are injectable; tests never inspect personal config.

mod config;
#[cfg(test)]
mod tests;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppTarget {
    ClaudeDesktop,
    ChatGpt,
}

impl AppTarget {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "Claude Desktop",
            Self::ChatGpt => "ChatGPT (Codex)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegistrationState {
    NotFound,
    Unregistered,
    Registered,
    DifferentTarget,
    Unreadable,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegistrationCommand {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

impl RegistrationCommand {
    pub(crate) fn snippet(&self, target: AppTarget) -> String {
        config::edit(target, "", Action::Register, self).expect("valid generated registration")
    }
}

/// Capture in the detection worker, then retain alongside its result for writes.
#[derive(Clone, Debug)]
pub(crate) struct RegistrationEnvironment {
    pub(crate) local_app_data: Option<PathBuf>,
    pub(crate) app_data: Option<PathBuf>,
    pub(crate) user_profile: Option<PathBuf>,
    pub(crate) codex_home: Option<PathBuf>,
    pub(crate) launcher_exe: Option<PathBuf>,
    pub(crate) current_exe: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) working_dir: PathBuf,
}

impl RegistrationEnvironment {
    pub(crate) fn capture() -> Result<Self, String> {
        let env_path = |name| std::env::var_os(name).map(PathBuf::from);
        Ok(Self {
            local_app_data: env_path("LOCALAPPDATA"),
            app_data: env_path("APPDATA"),
            user_profile: env_path("USERPROFILE"),
            codex_home: env_path("CODEX_HOME"),
            launcher_exe: env_path("MIV_LAUNCHER_EXE_PATH"),
            current_exe: std::env::current_exe().map_err(|_| "実行ファイルを確認できません")?,
            data_dir: mimageviewer_agent_ipc::resolve_data_dir(&crate::data_dir::get())
                .map_err(|_| "データフォルダを確認できません")?,
            working_dir: std::env::current_dir().map_err(|_| "作業フォルダを確認できません")?,
        })
    }

    fn absolute(&self, path: &Path) -> Result<PathBuf, String> {
        // Same lexical resolution as agent-ipc::resolve_data_dir, with an injected cwd.
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.working_dir.join(path)
        };
        if !path.is_absolute() {
            return Err("絶対パスを確認できません".into());
        }
        mimageviewer_agent_ipc::resolve_data_dir(&path)
            .map_err(|_| "絶対パスを確認できません".into())
    }

    pub(crate) fn resolve_command(&self) -> Result<RegistrationCommand, String> {
        let data = self.absolute(&self.data_dir)?;
        let (exe, mut args) = if let Some(launcher) = &self.launcher_exe {
            (self.absolute(launcher)?, vec!["--mcp".into()])
        } else {
            let core = self.absolute(&self.current_exe)?;
            let exe = core
                .parent()
                .ok_or("実行ファイルを確認できません")?
                .join("mimageviewer-mcp.exe");
            if !exe.is_file() {
                return Err("mimageviewer-mcp.exe が見つかりません".into());
            }
            (exe, Vec::new())
        };
        let as_text = |path: &Path| {
            path.to_str()
                .filter(|s| !s.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(|| "パスを登録できません".to_owned())
        };
        args.extend(["--data-dir".into(), as_text(&data)?]);
        Ok(RegistrationCommand {
            command: as_text(&exe)?,
            args,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AppRegistration {
    pub(crate) target: AppTarget,
    pub(crate) state: RegistrationState,
    pub(crate) config_paths: Vec<PathBuf>,
    pub(crate) message: Option<String>,
    pub(crate) changes: RegistrationChanges,
}

/// Small edit summary prepared on the detection worker, without config contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegistrationChange {
    Add,
    Replace,
    Remove,
    Unchanged,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RegistrationChanges {
    pub(crate) register: Option<RegistrationChange>,
    pub(crate) remove: Option<RegistrationChange>,
}

impl RegistrationChanges {
    pub(crate) fn from_config(
        target: AppTarget,
        text: &str,
        command: Option<&RegistrationCommand>,
    ) -> Result<Self, String> {
        let remove_command = RegistrationCommand {
            command: String::new(),
            args: Vec::new(),
        };
        Ok(Self {
            register: command
                .map(|command| config::change(target, text, Action::Register, command))
                .transpose()?,
            remove: Some(config::change(
                target,
                text,
                Action::Remove,
                &remove_command,
            )?),
        })
    }
}

impl AppRegistration {
    /// Availability requires an edit that succeeded on the detection worker.
    pub(crate) fn can_write(&self, action: Action) -> bool {
        !matches!(
            self.state,
            RegistrationState::NotFound
                | RegistrationState::Unreadable
                | RegistrationState::Ambiguous
        ) && self.config_paths.len() == 1
            && match action {
                Action::Register => self.changes.register.is_some(),
                Action::Remove => self.changes.remove.is_some(),
            }
    }

    pub(crate) fn can_remove(&self) -> bool {
        matches!(
            self.state,
            RegistrationState::Registered | RegistrationState::DifferentTarget
        ) && self.can_write(Action::Remove)
    }

    /// Removal does not need the currently installed MCP executable to exist.
    pub(crate) fn remove_request(&self) -> Option<WriteRequest> {
        if !self.can_remove() {
            return None;
        }
        self.write_request(
            Action::Remove,
            &RegistrationCommand {
                command: String::new(),
                args: Vec::new(),
            },
        )
    }

    pub(crate) fn write_request(
        &self,
        action: Action,
        command: &RegistrationCommand,
    ) -> Option<WriteRequest> {
        if !self.can_write(action) {
            return None;
        }
        Some(WriteRequest {
            target: self.target,
            path: self.config_paths[0].clone(),
            action,
            command: command.clone(),
            change: match action {
                Action::Register => self.changes.register?,
                Action::Remove => self.changes.remove?,
            },
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Detection {
    pub(crate) command: Result<RegistrationCommand, String>,
    pub(crate) apps: Vec<AppRegistration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Register,
    Remove,
}

/// Immutable target shown in the modal; write rereads its current bytes.
#[derive(Clone, Debug)]
pub(crate) struct WriteRequest {
    pub(crate) target: AppTarget,
    pub(crate) path: PathBuf,
    pub(crate) action: Action,
    pub(crate) command: RegistrationCommand,
    pub(crate) change: RegistrationChange,
}

fn packages(env: &RegistrationEnvironment, prefix: &str) -> io::Result<Vec<PathBuf>> {
    let Some(local) = &env.local_app_data else {
        return Ok(Vec::new());
    };
    let dir = env
        .absolute(local)
        .map_err(io::Error::other)?
        .join("Packages");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
            && entry.file_type()?.is_dir()
        {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

pub(crate) fn detect(env: &RegistrationEnvironment) -> Detection {
    let command = env.resolve_command();
    let apps = [AppTarget::ClaudeDesktop, AppTarget::ChatGpt].into_iter().map(|target| {
        let mut row = AppRegistration { target, state: RegistrationState::NotFound, config_paths: Vec::new(), message: None, changes: RegistrationChanges::default() };
        let result = (|| -> Result<(), String> {
            match target {
                AppTarget::ClaudeDesktop => {
                    if let Some(local) = &env.local_app_data {
                        if env.absolute(local)?.join("AnthropicClaude").is_dir() {
                            row.config_paths.push(env.absolute(env.app_data.as_deref().ok_or("設定フォルダを確認できません")?)?.join("Claude/claude_desktop_config.json"));
                        }
                    }
                    let packages = packages(env, "Claude_").map_err(|_| "インストール先を確認できません")?;
                    row.config_paths.extend(packages.into_iter().map(|p| p.join("LocalCache/Roaming/Claude/claude_desktop_config.json")));
                    if row.config_paths.len() > 1 {
                        row.state = RegistrationState::Ambiguous;
                        row.message = Some("Claude が複数の形で入っています。手順を見て、お使いの Claude の設定ファイルに書いてください".into());
                        return Ok(());
                    }
                }
                AppTarget::ChatGpt => {
                    let profile_codex = env.user_profile.as_deref().map(|p| env.absolute(p).map(|p| p.join(".codex"))).transpose()?;
                    if !packages(env, "OpenAI.Codex_").map_err(|_| "インストール先を確認できません")?.is_empty() || profile_codex.as_deref().is_some_and(Path::is_dir) {
                        let home = match &env.codex_home { Some(p) => env.absolute(p)?, None => profile_codex.ok_or("設定フォルダを確認できません")? };
                        row.config_paths.push(home.join("config.toml"));
                    }
                }
            }
            let Some(path) = row.config_paths.first() else { return Ok(()); };
            let text = read_config(path)?;
            row.state = config::state(target, &text, command.as_ref().ok())?;
            row.changes = RegistrationChanges::from_config(target, &text, command.as_ref().ok())?;
            Ok(())
        })();
        if let Err(error) = result { row.state = RegistrationState::Unreadable; row.message = Some(error); }
        row
    }).collect();
    Detection { command, apps }
}

fn read_config(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err("設定ファイルを読めません".into()),
    }
}

fn normalized_components(path: &Path) -> Vec<String> {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    let path = path.strip_prefix(r"unc\").unwrap_or(path);
    let mut parts: Vec<String> = Vec::new();
    for part in path.split('\\').filter(|part| !part.is_empty()) {
        match part {
            "." => {}
            ".." => {
                if parts.last().is_some_and(|part| !part.ends_with(':')) {
                    parts.pop();
                }
            }
            part => parts.push(part.to_owned()),
        }
    }
    parts
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunningApps {
    Stopped,
    App,
    CodexCliOrIde,
    AppAndCodexCliOrIde,
}

pub(crate) fn running_apps(
    env: &RegistrationEnvironment,
    target: AppTarget,
    images: &[(u32, PathBuf)],
) -> RunningApps {
    let squirrel = env
        .local_app_data
        .as_deref()
        .and_then(|p| env.absolute(p).ok())
        .map(|p| normalized_components(&p.join("AnthropicClaude")));
    let codex_daemon = env
        .codex_home
        .as_deref()
        .map(|home| env.absolute(home))
        .or_else(|| {
            env.user_profile
                .as_deref()
                .map(|profile| env.absolute(profile).map(|p| p.join(".codex")))
        })
        .and_then(Result::ok)
        .map(|home| normalized_components(&home.join("packages/app-server-daemon")));
    let mut app = false;
    let mut codex_cli_or_ide = false;
    for (_, image) in images {
        let parts = normalized_components(image);
        let windows_package = |prefix: &str| {
            parts
                .windows(2)
                .any(|p| p[0] == "windowsapps" && p[1].starts_with(prefix))
        };
        match target {
            AppTarget::ClaudeDesktop => {
                app |= squirrel
                    .as_ref()
                    .is_some_and(|root| parts.len() > root.len() && parts.starts_with(root))
                    || windows_package("claude_");
            }
            AppTarget::ChatGpt => {
                let is_daemon = codex_daemon
                    .as_ref()
                    .is_some_and(|root| parts.len() > root.len() && parts.starts_with(root));
                if !is_daemon {
                    // A package's codex.exe belongs to the desktop app. All
                    // other codex.exe paths retain the shared-config CLI gate.
                    if windows_package("openai.codex_") {
                        app = true;
                    } else if parts.last().is_some_and(|p| p == "codex.exe") {
                        codex_cli_or_ide = true;
                    }
                }
            }
        }
    }
    match (app, codex_cli_or_ide) {
        (false, false) => RunningApps::Stopped,
        (true, false) => RunningApps::App,
        (false, true) => RunningApps::CodexCliOrIde,
        (true, true) => RunningApps::AppAndCodexCliOrIde,
    }
}

pub(crate) fn write(
    env: &RegistrationEnvironment,
    request: &WriteRequest,
) -> Result<String, String> {
    let result = write_with_processes(env, request, crate::process_images::snapshot);
    if result.is_err() {
        // Never log parsed config, raw parser errors or configuration contents.
        crate::logger::log(
            "AI registration: operation failed; original configuration retained where possible",
        );
    }
    result
}

fn write_with_processes(
    env: &RegistrationEnvironment,
    request: &WriteRequest,
    processes: impl FnOnce() -> io::Result<Vec<(u32, PathBuf)>>,
) -> Result<String, String> {
    let images = processes().map_err(|_| "動作中のアプリを確認できません")?;
    let message = match (request.target, running_apps(env, request.target, &images)) {
        (_, RunningApps::Stopped) => None,
        (AppTarget::ClaudeDesktop, _) => Some("Claude Desktop を終了してから押してください"),
        (AppTarget::ChatGpt, RunningApps::App) => {
            Some("ChatGPT (Codex) を終了してから押してください")
        }
        (AppTarget::ChatGpt, RunningApps::CodexCliOrIde) => Some(
            "Codex CLI または IDE 拡張 (codex.exe) が動作中です。同じ設定ファイルを使うため、終了してから押してください",
        ),
        (AppTarget::ChatGpt, RunningApps::AppAndCodexCliOrIde) => Some(
            "ChatGPT (Codex) と、Codex CLI または IDE 拡張 (codex.exe) を終了してから押してください",
        ),
    };
    if let Some(message) = message {
        return Err(message.into());
    }
    if !request.path.is_absolute() {
        return Err("設定ファイルのパスを確認できません".into());
    }
    let original = read_config(&request.path)?;
    if config::change(request.target, &original, request.action, &request.command)?
        != request.change
    {
        return Err("設定ファイルが確認後に変わりました。もう一度確認してください".into());
    }
    let edited = config::edit(request.target, &original, request.action, &request.command)?;
    let parent = request
        .path
        .parent()
        .ok_or("設定フォルダを確認できません")?;
    fs::create_dir_all(parent).map_err(|_| "設定フォルダを作れません")?;
    if request.path.exists() {
        let mut backup = request.path.as_os_str().to_os_string();
        backup.push(".mimageviewer.bak");
        fs::write(PathBuf::from(backup), original.as_bytes())
            .map_err(|_| "設定のバックアップを作れません")?;
    }
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "設定の一時ファイルを作れません")?;
    temp.write_all(edited.as_bytes())
        .and_then(|_| temp.flush())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "設定を書き込めません")?;
    crate::archive_converter::replace_file_atomic(temp.path(), &request.path, false)
        .map_err(|_| "設定を置き換えられません")?;
    Ok(format!(
        "{} を起動して、新しいチャットで mIV の tool が出るか確かめてください",
        request.target.name()
    ))
}
