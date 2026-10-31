use super::*;

fn fixture() -> (tempfile::TempDir, RegistrationEnvironment) {
    let temp = tempfile::tempdir().unwrap();
    let env = RegistrationEnvironment {
        local_app_data: Some(temp.path().join("local")),
        app_data: Some(temp.path().join("roaming")),
        user_profile: Some(temp.path().join("user")),
        codex_home: None,
        launcher_exe: None,
        current_exe: temp.path().join("bin/mimageviewer-core.exe"),
        data_dir: PathBuf::from("data/../profile"),
        working_dir: temp.path().to_owned(),
    };
    fs::create_dir_all(env.current_exe.parent().unwrap()).unwrap();
    fs::write(
        env.current_exe
            .parent()
            .unwrap()
            .join("mimageviewer-mcp.exe"),
        "fixture",
    )
    .unwrap();
    (temp, env)
}

fn install_squirrel(env: &RegistrationEnvironment) {
    fs::create_dir_all(env.local_app_data.as_ref().unwrap().join("AnthropicClaude")).unwrap();
}

fn app(detection: &Detection, target: AppTarget) -> &AppRegistration {
    detection
        .apps
        .iter()
        .find(|row| row.target == target)
        .unwrap()
}

#[test]
fn commands_always_use_absolute_resolved_data_and_distribution_entrypoint() {
    let (_temp, mut env) = fixture();
    let command = env.resolve_command().unwrap();
    assert!(Path::new(&command.command).is_absolute());
    assert_eq!(
        command.args,
        vec![
            "--data-dir".to_owned(),
            env.working_dir.join("profile").to_str().unwrap().to_owned()
        ]
    );
    env.launcher_exe = Some(PathBuf::from("launcher/mimageviewer.exe"));
    let launcher = env.resolve_command().unwrap();
    assert_eq!(
        Path::new(&launcher.command),
        env.working_dir.join("launcher/mimageviewer.exe")
    );
    assert_eq!(launcher.args[0], "--mcp");
    assert_eq!(&launcher.args[1..], command.args);
    env.launcher_exe = None;
    fs::remove_file(command.command).unwrap();
    assert!(env.resolve_command().is_err());
}

#[test]
fn claude_installs_map_to_own_configs_and_ambiguity_is_not_writable() {
    let (_temp, env) = fixture();
    assert_eq!(
        app(&detect(&env), AppTarget::ClaudeDesktop).state,
        RegistrationState::NotFound
    );
    install_squirrel(&env);
    let d = detect(&env);
    let row = app(&d, AppTarget::ClaudeDesktop);
    assert_eq!(row.state, RegistrationState::Unregistered);
    assert_eq!(
        row.config_paths,
        vec![
            env.app_data
                .as_ref()
                .unwrap()
                .join("Claude/claude_desktop_config.json")
        ]
    );
    let package = env
        .local_app_data
        .as_ref()
        .unwrap()
        .join("Packages/Claude_one");
    fs::create_dir_all(&package).unwrap();
    let d = detect(&env);
    let row = app(&d, AppTarget::ClaudeDesktop);
    assert_eq!(row.state, RegistrationState::Ambiguous);
    assert_eq!(row.config_paths.len(), 2);
    assert!(
        row.write_request(Action::Register, d.command.as_ref().unwrap())
            .is_none()
    );
    fs::remove_dir(env.local_app_data.as_ref().unwrap().join("AnthropicClaude")).unwrap();
    let d = detect(&env);
    assert_eq!(
        app(&d, AppTarget::ClaudeDesktop).config_paths,
        vec![package.join("LocalCache/Roaming/Claude/claude_desktop_config.json")]
    );
    fs::create_dir_all(
        env.local_app_data
            .as_ref()
            .unwrap()
            .join("Packages/Claude_two"),
    )
    .unwrap();
    assert_eq!(
        app(&detect(&env), AppTarget::ClaudeDesktop).state,
        RegistrationState::Ambiguous
    );
}

#[test]
fn codex_detection_and_config_home_are_independent() {
    let (_temp, mut env) = fixture();
    env.codex_home = Some(env.working_dir.join("custom-codex"));
    fs::create_dir_all(env.codex_home.as_ref().unwrap()).unwrap();
    assert_eq!(
        app(&detect(&env), AppTarget::ChatGpt).state,
        RegistrationState::NotFound
    );
    fs::create_dir_all(
        env.local_app_data
            .as_ref()
            .unwrap()
            .join("Packages/OpenAI.Codex_one"),
    )
    .unwrap();
    assert_eq!(
        app(&detect(&env), AppTarget::ChatGpt).config_paths,
        vec![env.codex_home.as_ref().unwrap().join("config.toml")]
    );
    env.codex_home = None;
    assert_eq!(
        app(&detect(&env), AppTarget::ChatGpt).config_paths,
        vec![
            env.user_profile
                .as_ref()
                .unwrap()
                .join(".codex/config.toml")
        ]
    );
    fs::remove_dir(
        env.local_app_data
            .as_ref()
            .unwrap()
            .join("Packages/OpenAI.Codex_one"),
    )
    .unwrap();
    fs::create_dir_all(env.user_profile.as_ref().unwrap().join(".codex")).unwrap();
    assert_eq!(
        app(&detect(&env), AppTarget::ChatGpt).state,
        RegistrationState::Unregistered
    );
}

#[test]
fn process_paths_guard_desktop_and_shared_codex_without_confusing_claude_code() {
    let (_temp, env) = fixture();
    let images = |s: &str| vec![(10, PathBuf::from(s))];
    assert!(!app_is_running(
        &env,
        AppTarget::ClaudeDesktop,
        &images(r"C:\Users\a\.local\bin\claude.exe")
    ));
    assert!(!app_is_running(
        &env,
        AppTarget::ClaudeDesktop,
        &[(
            10,
            env.local_app_data
                .as_ref()
                .unwrap()
                .join("AnthropicClaude-other/claude.exe")
        )]
    ));
    assert!(app_is_running(
        &env,
        AppTarget::ClaudeDesktop,
        &[(
            10,
            env.local_app_data
                .as_ref()
                .unwrap()
                .join("AnthropicClaude/app-1/Claude.exe")
        )]
    ));
    assert!(app_is_running(
        &env,
        AppTarget::ClaudeDesktop,
        &images(r"C:\Program Files\WindowsApps\Claude_one\app\Claude.exe")
    ));
    assert!(app_is_running(
        &env,
        AppTarget::ChatGpt,
        &images(r"C:\Program Files\WindowsApps\OpenAI.Codex_one\app\ChatGPT.exe")
    ));
    assert!(app_is_running(
        &env,
        AppTarget::ChatGpt,
        &images(r"C:\tools\CODEX.EXE")
    ));
    assert!(!app_is_running(
        &env,
        AppTarget::ChatGpt,
        &images(r"C:\tools\codex-other.exe")
    ));
}

#[test]
fn json_edits_preserve_all_other_values_and_top_level_server_order() {
    let (_temp, env) = fixture();
    let command = env.resolve_command().unwrap();
    let original = r#"{"z":9,"mcpServers":{"zebra":{"args":[true,null,3.5]},"mimageviewer":{"command":"old"},"alpha":{}},"a":{"nested":[1,"two"]}}"#;
    let edited = config::edit(
        AppTarget::ClaudeDesktop,
        original,
        Action::Register,
        &command,
    )
    .unwrap();
    assert!(edited.find("\"z\"").unwrap() < edited.find("\"mcpServers\"").unwrap());
    assert!(edited.find("\"mcpServers\"").unwrap() < edited.find("\"a\"").unwrap());
    assert!(edited.find("\"zebra\"").unwrap() < edited.find("\"mimageviewer\"").unwrap());
    assert!(edited.find("\"mimageviewer\"").unwrap() < edited.find("\"alpha\"").unwrap());
    let before: serde_json::Value = serde_json::from_str(original).unwrap();
    let after: serde_json::Value = serde_json::from_str(&edited).unwrap();
    assert_eq!(before["z"], after["z"]);
    assert_eq!(before["a"], after["a"]);
    assert_eq!(before["mcpServers"]["zebra"], after["mcpServers"]["zebra"]);
    assert_eq!(
        config::state(AppTarget::ClaudeDesktop, &edited, Some(&command)).unwrap(),
        RegistrationState::Registered
    );
    assert_eq!(
        config::state(AppTarget::ClaudeDesktop, original, Some(&command)).unwrap(),
        RegistrationState::DifferentTarget
    );
    let removed =
        config::edit(AppTarget::ClaudeDesktop, &edited, Action::Remove, &command).unwrap();
    assert!(!removed.contains("mimageviewer"));
    assert!(removed.contains("zebra"));
    for initial in ["", "{}"] {
        assert_eq!(
            config::state(
                AppTarget::ClaudeDesktop,
                &config::edit(
                    AppTarget::ClaudeDesktop,
                    initial,
                    Action::Register,
                    &command
                )
                .unwrap(),
                Some(&command)
            )
            .unwrap(),
            RegistrationState::Registered
        );
    }
}

#[test]
fn toml_preserves_comments_other_tables_and_crlf_replaces_and_removes() {
    let (_temp, env) = fixture();
    let command = env.resolve_command().unwrap();
    let initial = "# top\r\nmodel = 'example' # model\r\n[mcp_servers.other]\r\ncommand = 'other' # keep\r\n[mcp_servers.mimageviewer]\r\ncommand = 'old'\r\nargs = []\r\n[other]\r\nvalue = 42 # tail\r\n";
    let edited = config::edit(AppTarget::ChatGpt, initial, Action::Register, &command).unwrap();
    for line in [
        "# top\r\n",
        "model = 'example' # model\r\n",
        "command = 'other' # keep\r\n",
        "value = 42 # tail\r\n",
    ] {
        assert!(edited.contains(line), "{edited}");
    }
    assert!(!edited.replace("\r\n", "").contains('\n'));
    assert!(edited.contains("default_tools_approval_mode = \"approve\""));
    assert_eq!(
        config::state(AppTarget::ChatGpt, &edited, Some(&command)).unwrap(),
        RegistrationState::Registered
    );
    let removed = config::edit(AppTarget::ChatGpt, &edited, Action::Remove, &command).unwrap();
    assert!(!removed.contains("mimageviewer"));
    assert!(removed.contains("# keep"));
    assert!(removed.contains("# tail"));
    let lf = config::edit(
        AppTarget::ChatGpt,
        "# LF\nmodel = 'x'\n",
        Action::Register,
        &command,
    )
    .unwrap();
    assert!(!lf.contains('\r'));
    let inline = config::edit(
        AppTarget::ChatGpt,
        "mcp_servers = { other = { command = 'other' } } # keep\n",
        Action::Register,
        &command,
    )
    .unwrap();
    assert_eq!(
        config::state(AppTarget::ChatGpt, &inline, Some(&command)).unwrap(),
        RegistrationState::Registered
    );
    assert!(inline.contains("# keep"));
    assert!(inline.contains("other"));
}

#[test]
fn removal_remains_available_when_the_mcp_executable_was_removed() {
    let (_temp, env) = fixture();
    install_squirrel(&env);
    let d = detect(&env);
    let request = app(&d, AppTarget::ClaudeDesktop)
        .write_request(Action::Register, d.command.as_ref().unwrap())
        .unwrap();
    write_with_processes(&env, &request, || Ok(vec![])).unwrap();
    fs::remove_file(&request.command.command).unwrap();
    let d = detect(&env);
    assert!(d.command.is_err());
    let request = app(&d, AppTarget::ClaudeDesktop).remove_request().unwrap();
    write_with_processes(&env, &request, || Ok(vec![])).unwrap();
    assert_eq!(
        app(&detect(&env), AppTarget::ClaudeDesktop).state,
        RegistrationState::Unregistered
    );
}

#[test]
fn real_file_reread_backup_replace_and_remove_use_only_injected_temporary_paths() {
    for target in [AppTarget::ClaudeDesktop, AppTarget::ChatGpt] {
        let (_temp, env) = fixture();
        install_squirrel(&env);
        fs::create_dir_all(env.user_profile.as_ref().unwrap().join(".codex")).unwrap();
        let d = detect(&env);
        let request = app(&d, target)
            .write_request(Action::Register, d.command.as_ref().unwrap())
            .unwrap();
        write_with_processes(&env, &request, || Ok(vec![])).unwrap();
        assert_eq!(
            app(&detect(&env), target).state,
            RegistrationState::Registered
        );
        let external = match target {
            AppTarget::ClaudeDesktop => "{\"new_external\":42}",
            AppTarget::ChatGpt => "# external\nexternal = 42\n",
        };
        fs::write(&request.path, external).unwrap();
        write_with_processes(&env, &request, || Ok(vec![])).unwrap();
        let mut backup = request.path.as_os_str().to_os_string();
        backup.push(".mimageviewer.bak");
        assert_eq!(fs::read_to_string(&backup).unwrap(), external);
        assert!(
            fs::read_to_string(&request.path)
                .unwrap()
                .contains("external")
        );
        let remove = WriteRequest {
            action: Action::Remove,
            ..request
        };
        write_with_processes(&env, &remove, || Ok(vec![])).unwrap();
        assert_eq!(
            app(&detect(&env), target).state,
            RegistrationState::Unregistered
        );
        assert!(
            fs::read_to_string(&remove.path)
                .unwrap()
                .contains("external")
        );
    }
}

#[test]
fn invalid_files_and_running_processes_never_change_config_or_backup() {
    for (target, invalid) in [
        (AppTarget::ClaudeDesktop, "{SECRET invalid"),
        (AppTarget::ChatGpt, "[SECRET invalid"),
    ] {
        let (_temp, env) = fixture();
        let path = env.working_dir.join("config");
        fs::write(&path, invalid).unwrap();
        let request = WriteRequest {
            target,
            path: path.clone(),
            action: Action::Register,
            command: env.resolve_command().unwrap(),
        };
        let error = write_with_processes(&env, &request, || Ok(vec![])).unwrap_err();
        assert!(!error.contains("SECRET"));
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        assert!(!env.working_dir.join("config.mimageviewer.bak").exists());
        fs::write(&path, "").unwrap();
        let image = match target {
            AppTarget::ClaudeDesktop => env
                .local_app_data
                .as_ref()
                .unwrap()
                .join("AnthropicClaude/Claude.exe"),
            AppTarget::ChatGpt => env.working_dir.join("codex.exe"),
        };
        assert!(write_with_processes(&env, &request, || Ok(vec![(1, image)])).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert!(!env.working_dir.join("config.mimageviewer.bak").exists());
        assert!(
            write_with_processes(&env, &request, || Err(io::Error::other("injected"))).is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }
}
