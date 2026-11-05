use super::*;

fn without_json_registration(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(servers) = value
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
    {
        servers.remove("mimageviewer");
        if servers.is_empty() {
            value.as_object_mut().unwrap().remove("mcpServers");
        }
    }
    value
}

#[test]
fn registration_preservation_json_values_across_formats_and_lifecycle() {
    let (_temp, env) = fixture();
    let command = env.resolve_command().unwrap();
    let fixture = r#"{"preferences":{"chromeExtension":{"enabled":true,"epitaxyPrefs":{"label":"日本語","emptyObject":{},"emptyArray":[],"none":null,"false":false,"integer":42}},"strings":["quote: \"","backslash: \\","newline: \n","é","escaped: \/"],"numbers":[0.1,1.1,1e-7,-2.5,9007199254740991,9007199254740992,9007199254740993,-9223372036854775808]},"tail":[true,3,"keep",{},[],null]}"#;
    let base: serde_json::Value = serde_json::from_str(fixture).unwrap();
    for servers in [
        None,
        Some(serde_json::json!({"other":{"command":"keep","args":["日本語"],"env":{"A":"B"}}})),
        Some(
            serde_json::json!({"other":{"command":"keep"},"mimageviewer":{"command":"old","extra":true}}),
        ),
    ] {
        let mut value = base.clone();
        if let Some(servers) = servers {
            value["mcpServers"] = servers;
        }
        for indent in ["  ", "    ", "\t", ""] {
            let mut bytes = Vec::new();
            if indent.is_empty() {
                serde_json::to_writer(&mut bytes, &value).unwrap();
            } else {
                let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
                let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
                serde::Serialize::serialize(&value, &mut serializer).unwrap();
            }
            let formatted = String::from_utf8(bytes).unwrap();
            for newline in ["\n", "\r\n"] {
                for trailing in [false, true] {
                    let mut original = formatted.replace('\n', newline);
                    if trailing {
                        original.push_str(newline);
                    }
                    let expected = without_json_registration(value.clone());
                    let mut text = original;
                    for action in [
                        Action::Register,
                        Action::Register,
                        Action::Remove,
                        Action::Register,
                        Action::Remove,
                        Action::Remove,
                    ] {
                        text = config::edit(AppTarget::ClaudeDesktop, &text, action, &command)
                            .unwrap();
                        let actual: serde_json::Value = serde_json::from_str(&text).unwrap();
                        assert_eq!(
                            without_json_registration(actual),
                            expected,
                            "indent={indent:?}, newline={newline:?}, trailing={trailing}, action={action:?}"
                        );
                    }
                }
            }
        }
    }
    // Keep the escaped spellings and exponent form in the input too; the
    // formatting matrix above deliberately starts from parsed JSON values.
    for action in [Action::Register, Action::Remove] {
        let edited = config::edit(AppTarget::ClaudeDesktop, fixture, action, &command).unwrap();
        assert_eq!(
            without_json_registration(serde_json::from_str(&edited).unwrap()),
            base
        );
    }
}

fn toml_semantic_value(item: &toml_edit::Item) -> serde_json::Value {
    if let Some(tables) = item.as_array_of_tables() {
        return serde_json::Value::Array(
            tables
                .iter()
                .map(|table| {
                    serde_json::Value::Object(
                        table
                            .iter()
                            .map(|(key, item)| (key.to_owned(), toml_semantic_value(item)))
                            .collect(),
                    )
                })
                .collect(),
        );
    }
    if let Some(table) = item.as_table_like() {
        return serde_json::Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.to_owned(), toml_semantic_value(item)))
                .collect(),
        );
    }
    fn convert(value: &toml_edit::Value) -> serde_json::Value {
        match value {
            toml_edit::Value::String(v) => serde_json::json!(v.value()),
            toml_edit::Value::Integer(v) => serde_json::json!(v.value()),
            toml_edit::Value::Float(v) => serde_json::json!(v.value()),
            toml_edit::Value::Boolean(v) => serde_json::json!(v.value()),
            toml_edit::Value::Datetime(v) => serde_json::json!(v.value().to_string()),
            toml_edit::Value::Array(v) => serde_json::Value::Array(v.iter().map(convert).collect()),
            toml_edit::Value::InlineTable(v) => serde_json::Value::Object(
                v.iter().map(|(k, v)| (k.to_owned(), convert(v))).collect(),
            ),
        }
    }
    if let Some(v) = item.as_value() {
        convert(v)
    } else {
        serde_json::Value::Null
    }
}

fn without_toml_registration(text: &str) -> serde_json::Value {
    let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
    if let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        servers.remove("mimageviewer");
        if servers.is_empty() {
            doc.remove("mcp_servers");
        }
    }
    toml_semantic_value(doc.as_item())
}

#[test]
fn registration_preservation_toml_values_comments_blank_lines_and_crlf() {
    let (_temp, env) = fixture();
    let command = env.resolve_command().unwrap();
    let prefix = "# retained top\nmodel = '日本語' # retained model\n\n[preferences]\nenabled = true\ninteger = -9223372036854775808\nfloat = 0.1\nvalues = ['é', 9007199254740993, false, { empty = [] }]\n\n# retained other\n[mcp_servers.other]\ncommand = 'keep' # retained command\nargs = ['quote: \"', 'backslash: \\']\n\n";
    let suffix = "[tail]\nvalue = 42 # retained tail\nnumbers = [0.1, 1.1, 1e-7, -2.5, 9007199254740991, 9007199254740992]\n\n[[skills.config]]\nname = '日本語é'\nenabled = true\n\n[[skills.config]]\nname = 'second'\nenabled = false\n\n";
    for existing in [
        "",
        "[mcp_servers.mimageviewer]\ncommand = 'old'\nargs = []\n\n",
    ] {
        for newline in ["\n", "\r\n"] {
            let original = format!("{prefix}{existing}{suffix}").replace('\n', newline);
            let expected = without_toml_registration(&original);
            let mut text = original;
            for action in [
                Action::Register,
                Action::Register,
                Action::Remove,
                Action::Remove,
            ] {
                text = config::edit(AppTarget::ChatGpt, &text, action, &command).unwrap();
                assert_eq!(without_toml_registration(&text), expected);
                assert!(text.starts_with(&prefix.replace('\n', newline)), "{text}");
                assert!(text.contains(&suffix.replace('\n', newline)), "{text}");
                if newline == "\r\n" {
                    assert!(!text.replace("\r\n", "").contains('\n'));
                }
            }
        }
    }
    for original in [
        "",
        "# no MCP parent\nmodel = 'keep'\n\n",
        "mcp_servers = { other = { command = 'keep' } } # inline\n",
    ] {
        let registered =
            config::edit(AppTarget::ChatGpt, original, Action::Register, &command).unwrap();
        let removed =
            config::edit(AppTarget::ChatGpt, &registered, Action::Remove, &command).unwrap();
        assert_eq!(
            without_toml_registration(original),
            without_toml_registration(&registered)
        );
        assert_eq!(
            without_toml_registration(original),
            without_toml_registration(&removed)
        );
    }
}

#[test]
fn json_registration_preserves_pretty_indentation_line_endings_and_other_bytes() {
    let command = RegistrationCommand {
        command: r"C:\日本語\mimageviewer.exe".into(),
        args: vec!["--mcp".into(), "--data-dir".into(), r"C:\profile".into()],
    };
    // Canonical JSON.stringify-style scalar spellings: compare complete text so
    // unrelated values, number spellings, order and whitespace must all survive.
    let base = r#"{
  "preferences": {
    "chromeExtension": true,
    "epitaxyPrefs": {
      "name": "日本語é",
      "values": [
        0.1,
        1.1,
        1e-7,
        -2.5,
        9007199254740991,
        9007199254740992,
        "quote: \" backslash: \\ newline: \n",
        {},
        [],
        null
      ]
    }
  },
  "mcpServers": {
    "other": {
      "command": "keep",
      "args": []
    }
  },
  "tail": true
}"#;
    let registration = format!(
        "    \"mimageviewer\": {{\n      \"command\": {},\n      \"args\": [\n{}\n      ]\n    }}",
        serde_json::to_string(&command.command).unwrap(),
        command
            .args
            .iter()
            .map(|arg| format!("        {}", serde_json::to_string(arg).unwrap()))
            .collect::<Vec<_>>()
            .join(",\n"),
    );
    let parent = "  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"keep\",\n      \"args\": []\n    }\n  },\n";
    let with_registration = base.replace(
        "      \"args\": []\n    }\n  },",
        &format!("      \"args\": []\n    }},\n{registration}\n  }},"),
    );
    let old_registration =
        "    \"mimageviewer\": {\n      \"command\": \"old\",\n      \"args\": []\n    }";
    for indent in ["  ", "    ", "\t"] {
        for newline in ["\n", "\r\n"] {
            for trailing in [false, true] {
                let render = |text: &str| {
                    let mut output = text
                        .lines()
                        .map(|line| {
                            let spaces = line.bytes().take_while(|b| *b == b' ').count();
                            format!("{}{}", indent.repeat(spaces / 2), &line[spaces..])
                        })
                        .collect::<Vec<_>>()
                        .join(newline);
                    if trailing {
                        output.push_str(newline);
                    }
                    output
                };
                for existing in [false, true] {
                    let original = if existing {
                        with_registration.replace(&registration, old_registration)
                    } else {
                        base.to_owned()
                    };
                    let edited = config::edit(
                        AppTarget::ClaudeDesktop,
                        &render(&original),
                        Action::Register,
                        &command,
                    )
                    .unwrap();
                    assert_eq!(edited, render(&with_registration));
                    assert_eq!(
                        config::edit(
                            AppTarget::ClaudeDesktop,
                            &edited,
                            Action::Register,
                            &command
                        )
                        .unwrap(),
                        edited
                    );
                    let removed =
                        config::edit(AppTarget::ClaudeDesktop, &edited, Action::Remove, &command)
                            .unwrap();
                    assert_eq!(removed, render(base));
                    if existing {
                        assert_eq!(
                            config::edit(
                                AppTarget::ClaudeDesktop,
                                &render(&original),
                                Action::Remove,
                                &command
                            )
                            .unwrap(),
                            render(base)
                        );
                    }
                }
                let no_parent = base.replace(parent, "");
                let added_parent = no_parent.replace(
                    "  \"tail\": true\n}",
                    &format!("  \"tail\": true,\n  \"mcpServers\": {{\n{registration}\n  }}\n}}"),
                );
                let edited = config::edit(
                    AppTarget::ClaudeDesktop,
                    &render(&no_parent),
                    Action::Register,
                    &command,
                )
                .unwrap();
                assert_eq!(edited, render(&added_parent));
                let empty_parent = no_parent.replace(
                    "  \"tail\": true\n}",
                    "  \"tail\": true,\n  \"mcpServers\": {}\n}",
                );
                assert_eq!(
                    config::edit(AppTarget::ClaudeDesktop, &edited, Action::Remove, &command)
                        .unwrap(),
                    render(&empty_parent)
                );
            }
        }
    }
    for empty in ["", " \r\n\t"] {
        let added =
            config::edit(AppTarget::ClaudeDesktop, empty, Action::Register, &command).unwrap();
        assert!(added.ends_with(if empty.contains("\r\n") { "\r\n" } else { "\n" }));
    }
    let minified =
        config::edit(AppTarget::ClaudeDesktop, "{}", Action::Register, &command).unwrap();
    assert!(minified.starts_with("{\n  \"mcpServers\""));
    assert!(!minified.ends_with('\n'));
}

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
fn codex_daemon_exclusion_uses_normalized_components_under_the_current_home_only() {
    let (_temp, mut env) = fixture();
    let profile = env.user_profile.as_ref().unwrap().clone();
    let default_home = profile.join(".codex");
    let running = |env: &RegistrationEnvironment, path: PathBuf| {
        app_is_running(env, AppTarget::ChatGpt, &[(10, path)])
    };
    let daemon = default_home.join("packages/app-server-daemon/1/bin/codex.exe");
    assert!(!running(&env, daemon.clone()));
    assert!(!running(
        &env,
        PathBuf::from(daemon.to_str().unwrap().replace('\\', "/").to_uppercase())
    ));
    assert!(!running(
        &env,
        PathBuf::from(format!(r"\\?\{}", daemon.display()))
    ));
    assert!(!running(
        &env,
        default_home.join("packages/unused/../app-server-daemon/./1/codex.exe")
    ));
    for path in [
        default_home.join("packages/app-server-daemon-other/1/codex.exe"),
        default_home.join("packages/app-server-daemon.exe/codex.exe"),
        default_home.join("packages/app-server-daemon/../other/codex.exe"),
        default_home.join("packages-other/app-server-daemon/1/codex.exe"),
        profile.join(".codex-other/packages/app-server-daemon/1/codex.exe"),
        env.working_dir
            .join("another-profile/.codex/packages/app-server-daemon/1/codex.exe"),
        env.local_app_data
            .as_ref()
            .unwrap()
            .join("OpenAI/Codex/bin/codex.exe"),
        PathBuf::from(r"C:\tools\codex.exe"),
        PathBuf::from(r"C:\Program Files\WindowsApps\OpenAI.Codex_one\app\ChatGPT.exe"),
    ] {
        assert!(running(&env, path.clone()), "{}", path.display());
    }

    // A relative CODEX_HOME uses the same injected cwd resolution as registration.
    env.codex_home = Some(PathBuf::from("custom/../custom-codex"));
    let custom_home = env.working_dir.join("custom-codex");
    assert!(!running(
        &env,
        custom_home.join("packages/app-server-daemon/1/codex.exe")
    ));
    assert!(running(&env, daemon));
    assert!(running(
        &env,
        env.working_dir
            .join("custom-codex-other/packages/app-server-daemon/1/codex.exe")
    ));
    assert!(running(&env, custom_home.join("bin/codex.exe")));
    env.user_profile = None;
    assert!(!running(
        &env,
        custom_home.join("packages/app-server-daemon/1/codex.exe")
    ));
    env.codex_home = None;
    assert!(running(
        &env,
        custom_home.join("packages/app-server-daemon/1/codex.exe")
    ));
}

#[test]
fn registration_changes_match_editor_for_add_replace_remove_and_unchanged() {
    let (_temp, env) = fixture();
    let command = env.resolve_command().unwrap();
    for (target, originals) in [
        (
            AppTarget::ChatGpt,
            vec![
                "",
                "# keep\nmodel = 'x'\n",
                "# keep\r\n[mcp_servers.other]\r\ncommand = 'other' # retain\r\n",
                "[mcp_servers]\n# explicit parent\n[mcp_servers.other]\ncommand = 'other'\n",
                "mcp_servers = { other = { command = 'other' } } # inline\n",
            ],
        ),
        (
            AppTarget::ClaudeDesktop,
            vec![
                "",
                "{}",
                r#"{"other":42,"mcpServers":{"another":{"command":"keep"}}}"#,
            ],
        ),
    ] {
        for original in originals {
            let added = config::edit(target, original, Action::Register, &command).unwrap();
            let changes =
                RegistrationChanges::from_config(target, original, Some(&command)).unwrap();
            assert_eq!(changes.register, Some(RegistrationChange::Add));
            assert_eq!(changes.remove, Some(RegistrationChange::Unchanged));
            assert_eq!(
                config::state(target, &added, Some(&command)).unwrap(),
                RegistrationState::Registered
            );
            assert_eq!(
                config::edit(target, &added, Action::Register, &command).unwrap(),
                added
            );
            let changes = RegistrationChanges::from_config(target, &added, Some(&command)).unwrap();
            assert_eq!(changes.register, Some(RegistrationChange::Unchanged));
            assert_eq!(changes.remove, Some(RegistrationChange::Remove));
            let changes = RegistrationChanges::from_config(target, &added, None).unwrap();
            assert_eq!(changes.register, None);
            assert_eq!(changes.remove, Some(RegistrationChange::Remove));
            let removed = config::edit(target, &added, Action::Remove, &command).unwrap();
            assert_eq!(
                config::state(target, &removed, Some(&command)).unwrap(),
                RegistrationState::Unregistered
            );
            assert_eq!(
                config::edit(target, &removed, Action::Remove, &command).unwrap(),
                removed
            );
            assert_eq!(
                config::change(target, &removed, Action::Remove, &command).unwrap(),
                RegistrationChange::Unchanged
            );
        }
    }
    for (target, original) in [
        (
            AppTarget::ChatGpt,
            "[mcp_servers.mimageviewer]\ncommand = 'old' # removed\nargs = []\n[other]\nvalue = 42 # keep\n",
        ),
        (
            AppTarget::ClaudeDesktop,
            r#"{"mcpServers":{"mimageviewer":{"command":"old","extra":true},"other":{}},"tail":42}"#,
        ),
    ] {
        let changes = RegistrationChanges::from_config(target, original, Some(&command)).unwrap();
        assert_eq!(changes.register, Some(RegistrationChange::Replace));
        assert_eq!(changes.remove, Some(RegistrationChange::Remove));
        let replaced = config::edit(target, original, Action::Register, &command).unwrap();
        assert_eq!(
            config::state(target, &replaced, Some(&command)).unwrap(),
            RegistrationState::Registered
        );
    }
    for (target, invalid) in [
        (AppTarget::ChatGpt, "[invalid"),
        (AppTarget::ClaudeDesktop, "{invalid"),
    ] {
        assert!(RegistrationChanges::from_config(target, invalid, Some(&command)).is_err());
        assert!(config::change(target, invalid, Action::Remove, &command).is_err());
    }
}

#[test]
fn daemon_images_allow_writes_but_a_cli_image_in_the_same_snapshot_still_blocks() {
    let (_temp, env) = fixture();
    fs::create_dir_all(env.user_profile.as_ref().unwrap().join(".codex")).unwrap();
    let detection = detect(&env);
    let row = app(&detection, AppTarget::ChatGpt);
    let request = row
        .write_request(Action::Register, detection.command.as_ref().unwrap())
        .unwrap();
    let daemon = env
        .user_profile
        .as_ref()
        .unwrap()
        .join(".codex/packages/app-server-daemon/1/codex.exe");
    write_with_processes(&env, &request, || Ok(vec![(10, daemon.clone())])).unwrap();
    let written = fs::read_to_string(&request.path).unwrap();
    assert_eq!(request.change, RegistrationChange::Add);
    assert_eq!(
        config::state(AppTarget::ChatGpt, &written, Some(&request.command)).unwrap(),
        RegistrationState::Registered
    );
    let detection = detect(&env);
    let remove = app(&detection, AppTarget::ChatGpt)
        .remove_request()
        .unwrap();
    assert!(
        write_with_processes(&env, &remove, || {
            Ok(vec![
                (10, daemon.clone()),
                (11, env.working_dir.join("cli/codex.exe")),
            ])
        })
        .is_err()
    );
    assert_eq!(fs::read_to_string(&request.path).unwrap(), written);
    write_with_processes(&env, &remove, || Ok(vec![(10, daemon)])).unwrap();
    assert_eq!(remove.change, RegistrationChange::Remove);
    assert_eq!(
        config::state(
            AppTarget::ChatGpt,
            &fs::read_to_string(&request.path).unwrap(),
            None
        )
        .unwrap(),
        RegistrationState::Unregistered
    );
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
            change: RegistrationChange::Add,
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
