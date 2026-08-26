use super::*;

fn install_matches(args: &[&str]) -> ArgMatches {
    let mut argv = vec!["ai", "install", "claude"];
    argv.extend_from_slice(args);
    build_ai_command()
        .try_get_matches_from(argv)
        .expect("ai install arguments should parse")
        .subcommand_matches("install")
        .expect("install subcommand should match")
        .clone()
}

fn text_options() -> GlobalOptions {
    GlobalOptions {
        output: OutputMode::Text,
        quiet: false,
        limit: None,
        cwd: None,
    }
}

#[test]
fn a_bare_install_carries_no_decision_flags() {
    assert!(!has_decision_flags(&install_matches(&[]), &text_options()));
}

#[test]
fn every_decision_flag_disables_prompting() {
    for args in [
        vec!["--scope", "user"],
        vec!["--transport", "http"],
        vec!["--transport", "stdio"],
        vec!["--mcp-only"],
        vec!["--rules-only"],
        vec!["--dry-run"],
        vec!["--yes"],
    ] {
        assert!(
            has_decision_flags(&install_matches(&args), &text_options()),
            "{args:?} must disable prompting"
        );
    }
}

#[test]
fn machine_readable_and_quiet_output_also_disable_prompting() {
    let json = GlobalOptions {
        output: OutputMode::Json,
        quiet: false,
        limit: None,
        cwd: None,
    };
    let quiet = GlobalOptions {
        output: OutputMode::Text,
        quiet: true,
        limit: None,
        cwd: None,
    };
    assert!(has_decision_flags(&install_matches(&[]), &json));
    assert!(has_decision_flags(&install_matches(&[]), &quiet));
}

#[test]
fn an_explicit_stdio_transport_is_distinguished_from_the_default() {
    assert_eq!(
        install_matches(&[]).value_source("transport"),
        Some(ValueSource::DefaultValue),
        "the default must not look like an explicit choice"
    );
    assert_eq!(
        install_matches(&["--transport", "stdio"]).value_source("transport"),
        Some(ValueSource::CommandLine)
    );
}

#[test]
fn managed_transport_sets_the_provisioning_flag_and_stays_http() {
    let raw_args = [
        OsString::from("ah"),
        OsString::from("ai"),
        OsString::from("install"),
        OsString::from("claude"),
        OsString::from("--transport"),
        OsString::from("managed"),
    ]
    .to_vec();
    let parsed = parse_runtime_command(raw_args, &[]).expect("managed transport should parse");
    let CliParseResult::Command(RuntimeCommand::Ai {
        request: crate::ai::install::AiCommand::Install(request),
        ..
    }) = parsed
    else {
        panic!("expected an ai install command");
    };
    assert!(request.managed);
    assert_eq!(request.transport, Transport::Http);
}

#[test]
fn secrets_redaction_skips_global_options_while_locating_action() {
    let raw_args = [
        "ah",
        "--cwd",
        "workspace",
        "secrets",
        "--limit",
        "10",
        "add",
        "billing",
        "--kind",
        "postgres",
        "hunter2",
    ]
    .map(OsString::from);

    let sanitized = redact_secret_command_argv(&raw_args);

    assert_eq!(sanitized.last(), Some(&OsString::from("[REDACTED]")));
    assert_eq!(sanitized[2], "workspace");
    assert_eq!(sanitized[5], "10");
}

#[test]
fn secrets_redaction_covers_values_that_look_like_options() {
    let raw_args = [
        "ah",
        "secrets",
        "add",
        "billing",
        "--kind",
        "postgres",
        "--label",
        "Billing",
        "--open",
        "--password",
        "-hunter2",
    ]
    .map(OsString::from);

    let sanitized = redact_secret_command_argv(&raw_args);

    assert!(!sanitized.iter().any(|value| value == "-hunter2"));
    assert_eq!(sanitized.last(), Some(&OsString::from("[REDACTED]")));
    // Known metadata options and their values stay readable in the log.
    assert_eq!(sanitized[7], "Billing");
    assert_eq!(sanitized[8], "--open");
}

#[test]
fn help_includes_dynamic_plugin_domain() {
    let plugins = vec![
        PluginMetadata {
            plugin_name: "builtin-file".to_owned(),
            domain: "file".to_owned(),
            description: "File operations plugin (built-in)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        },
        PluginMetadata {
            plugin_name: "external-ollama".to_owned(),
            domain: "ollama".to_owned(),
            description: "Ollama Local API plugin (dynamic)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        },
    ];
    let mut command = build_cli_command(&plugins);
    let mut out = Vec::new();
    command
        .write_long_help(&mut out)
        .expect("long help should render");
    let help_text = String::from_utf8(out).expect("help must be valid utf8");
    assert!(help_text.contains("file"));
    assert!(help_text.contains("ollama"));
    assert!(help_text.contains("Ollama Local API plugin (dynamic)"));
}

#[test]
fn parser_routes_dynamic_domain_to_invoke() {
    let plugins = vec![PluginMetadata {
        plugin_name: "external-ollama".to_owned(),
        domain: "ollama".to_owned(),
        description: "Ollama Local API plugin (dynamic)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }];
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("ollama"),
        OsString::from("ask"),
        OsString::from("--model"),
        OsString::from("llama3.2"),
        OsString::from("--prompt"),
        OsString::from("ping"),
    ];
    let parsed = parse_runtime_command(raw_args, &plugins).expect("parse should succeed");
    let CliParseResult::Command(RuntimeCommand::Invoke { domain, argv, .. }) = parsed else {
        panic!("unexpected parse result")
    };
    assert_eq!(domain, "ollama");
    assert_eq!(argv, vec!["ask", "--model", "llama3.2", "--prompt", "ping"]);
}

#[test]
fn parser_routes_upgrade_check_as_a_host_command() {
    let parsed = parse_runtime_command(
        vec![
            OsString::from("ah"),
            OsString::from("upgrade"),
            OsString::from("--check"),
        ],
        &[],
    )
    .unwrap();
    let CliParseResult::Command(RuntimeCommand::Upgrade { request, .. }) = parsed else {
        panic!("unexpected parse result")
    };
    assert_eq!(request, ah_updater::request::UpgradeRequest::Check);
}

#[test]
fn parser_routes_secrets_add_metadata_without_values() {
    let parsed = parse_runtime_command(
        vec![
            OsString::from("ah"),
            OsString::from("secrets"),
            OsString::from("add"),
            OsString::from("billing"),
            OsString::from("--kind"),
            OsString::from("postgres"),
            OsString::from("--label"),
            OsString::from("Billing"),
            OsString::from("--description"),
            OsString::from("Production billing database"),
            OsString::from("--open"),
        ],
        &[],
    )
    .unwrap();
    let CliParseResult::Command(RuntimeCommand::Secrets { request, .. }) = parsed else {
        panic!("unexpected parse result")
    };
    let crate::commands::secrets::SecretsCommand::Add {
        id,
        kind,
        label,
        description,
        open,
    } = request
    else {
        panic!("unexpected secrets command")
    };
    assert_eq!(id, "billing");
    assert_eq!(kind, crate::secrets::SecretKind::Postgres);
    assert_eq!(label.as_deref(), Some("Billing"));
    assert_eq!(description.as_deref(), Some("Production billing database"));
    assert!(open);
}

#[test]
fn parser_rejects_secrets_values_in_argv() {
    let secret = "must-not-enter-runtime-command";
    let error = parse_runtime_command(
        vec![
            OsString::from("ah"),
            OsString::from("secrets"),
            OsString::from("add"),
            OsString::from("billing"),
            OsString::from("--kind"),
            OsString::from("postgres"),
            OsString::from("--password"),
            OsString::from(secret),
        ],
        &[],
    )
    .err()
    .expect("secret argv values must be rejected");
    assert!(!error.detail_message().contains(secret));
}

#[test]
fn parser_strips_invocation_globals_from_domain_argv() {
    let plugins = vec![PluginMetadata {
        plugin_name: "external-ollama".to_owned(),
        domain: "ollama".to_owned(),
        description: "Ollama Local API plugin (dynamic)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }];
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("ollama"),
        OsString::from("ask"),
        OsString::from("--json"),
        OsString::from("--quiet"),
        OsString::from("--limit"),
        OsString::from("3"),
        OsString::from("--prompt"),
        OsString::from("ping"),
    ];
    let parsed = parse_runtime_command(raw_args, &plugins).expect("parse should succeed");
    let CliParseResult::Command(RuntimeCommand::Invoke {
        domain,
        argv,
        options,
    }) = parsed
    else {
        panic!("unexpected parse result");
    };
    assert_eq!(domain, "ollama");
    assert_eq!(options.output, OutputMode::Json);
    assert!(options.quiet);
    assert_eq!(options.limit, Some(3));
    assert_eq!(argv, vec!["ask", "--prompt", "ping"]);
}

#[test]
fn parser_routes_mcp_stdio_server_with_defaults() {
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("mcp"),
        OsString::from("serve"),
    ];
    let parsed = parse_runtime_command(raw_args, &[]).expect("mcp serve should parse");
    let CliParseResult::Command(RuntimeCommand::McpServe {
        transport,
        port,
        max_active,
        default_timeout_ms,
        options,
    }) = parsed
    else {
        panic!("unexpected parse result");
    };
    assert_eq!(transport, McpTransport::Stdio);
    assert_eq!(port, 8787);
    assert_eq!(max_active, 32);
    assert_eq!(default_timeout_ms, 300_000);
    assert_eq!(options.limit, None);
    assert!(!options.quiet);
}

#[test]
fn parser_routes_mcp_http_server() {
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("mcp"),
        OsString::from("serve"),
        OsString::from("--transport"),
        OsString::from("http"),
        OsString::from("--port"),
        OsString::from("9123"),
        OsString::from("--max-active"),
        OsString::from("7"),
    ];
    let parsed = parse_runtime_command(raw_args, &[]).expect("HTTP MCP serve should parse");
    let CliParseResult::Command(RuntimeCommand::McpServe {
        transport,
        port,
        max_active,
        ..
    }) = parsed
    else {
        panic!("unexpected parse result");
    };
    assert_eq!(transport, McpTransport::Http);
    assert_eq!(port, 9123);
    assert_eq!(max_active, 7);
}

#[test]
fn parser_rejects_removed_max_queued_with_migration_hint() {
    for raw_args in [
        vec![
            OsString::from("ah"),
            OsString::from("mcp"),
            OsString::from("serve"),
            OsString::from("--max-queued"),
            OsString::from("2"),
        ],
        vec![
            OsString::from("ah"),
            OsString::from("mcp"),
            OsString::from("serve"),
            OsString::from("--max-queued=2"),
        ],
    ] {
        let error = parse_runtime_command(raw_args, &[])
            .err()
            .expect("removed option must fail");
        assert!(error.detail_message().contains("--max-active"));
    }
}

#[test]
fn parser_forwards_max_queued_to_plugin() {
    let plugins = vec![PluginMetadata {
        plugin_name: "external-ollama".to_owned(),
        domain: "ollama".to_owned(),
        description: "Ollama Local API plugin (dynamic)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }];
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("ollama"),
        OsString::from("ask"),
        OsString::from("--max-queued"),
        OsString::from("2"),
    ];

    let parsed = parse_runtime_command(raw_args, &plugins).expect("plugin args should parse");
    let CliParseResult::Command(RuntimeCommand::Invoke { argv, .. }) = parsed else {
        panic!("unexpected parse result");
    };
    assert_eq!(argv, vec!["ask", "--max-queued", "2"]);
}

#[test]
fn parser_forwards_max_queued_to_run_check_child() {
    let plugins = vec![PluginMetadata {
        plugin_name: "builtin-run".to_owned(),
        domain: "run".to_owned(),
        description: "Command execution check utilities".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }];
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("run"),
        OsString::from("check"),
        OsString::from("child"),
        OsString::from("--max-queued"),
        OsString::from("2"),
    ];

    let parsed = parse_runtime_command(raw_args, &plugins).expect("child args should parse");
    let CliParseResult::Command(RuntimeCommand::Invoke { argv, .. }) = parsed else {
        panic!("unexpected parse result");
    };
    assert_eq!(argv, vec!["check", "--", "child", "--max-queued", "2"]);
}

#[test]
fn parser_rejects_json_for_mcp_stdio_server() {
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("--json"),
        OsString::from("mcp"),
        OsString::from("serve"),
    ];
    let error = parse_runtime_command(raw_args, &[])
        .err()
        .expect("--json must be rejected");
    assert!(
        error
            .detail_message()
            .contains("stdout is the MCP transport")
    );
}

#[test]
fn extract_last_cwd_supports_equals_form() {
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("file"),
        OsString::from("read"),
        OsString::from("a.txt"),
        OsString::from("--cwd=tmp/workdir"),
    ];
    let cwd = extract_last_cwd(&raw_args).expect("cwd extraction should succeed");
    assert_eq!(cwd, Some(PathBuf::from("tmp/workdir")));
}

#[test]
fn extract_last_cwd_ignores_run_check_child_arguments() {
    let raw_args = vec![
        OsString::from("ah"),
        OsString::from("--cwd=workspace"),
        OsString::from("run"),
        OsString::from("check"),
        OsString::from("child"),
        OsString::from("--cwd"),
        OsString::from("nested"),
    ];

    let cwd = extract_last_cwd(&raw_args).expect("cwd extraction should succeed");
    assert_eq!(cwd, Some(PathBuf::from("workspace")));
}

#[test]
fn prepare_run_check_preserves_opaque_child_suffix() {
    let mut raw_args = vec![
        OsString::from("ah"),
        OsString::from("--json"),
        OsString::from("run"),
        OsString::from("check"),
        OsString::from("--timeout-secs"),
        OsString::from("5"),
        OsString::from("child"),
        OsString::from("--json"),
        OsString::from("--limit"),
        OsString::from("not-a-host-limit"),
        OsString::from("--cwd"),
        OsString::from("nested"),
    ];

    let argv = prepare_run_check_passthrough(&mut raw_args)
        .expect("passthrough preparation should succeed")
        .expect("run check should be detected");
    assert_eq!(
        argv,
        vec![
            "check",
            "--timeout-secs",
            "5",
            "--",
            "child",
            "--json",
            "--limit",
            "not-a-host-limit",
            "--cwd",
            "nested",
        ]
    );
    assert_eq!(raw_args[6], "__ah_opaque_child_arg_0__");
    assert_eq!(raw_args[11], "__ah_opaque_child_arg_5__");
}

#[test]
fn parser_parses_plugins_state_management_commands() {
    let plugins = vec![PluginMetadata {
        plugin_name: "builtin-http".to_owned(),
        domain: "http".to_owned(),
        description: "HTTP workflow plugin (built-in)".to_owned(),
        abi_version: 1,
        required_tools: Vec::new(),
        compatibility: Default::default(),
    }];

    let disable_args = vec![
        OsString::from("ah"),
        OsString::from("plugins"),
        OsString::from("disable"),
        OsString::from("http"),
    ];
    let parsed_disable =
        parse_runtime_command(disable_args, &plugins).expect("disable should parse");
    let CliParseResult::Command(RuntimeCommand::PluginsDisable { domain, .. }) = parsed_disable
    else {
        panic!("unexpected disable parse result");
    };
    assert_eq!(domain, "http");

    let list_args = vec![
        OsString::from("ah"),
        OsString::from("plugins"),
        OsString::from("list"),
        OsString::from("--state"),
        OsString::from("disabled"),
    ];
    let parsed_list = parse_runtime_command(list_args, &plugins).expect("list should parse");
    let CliParseResult::Command(RuntimeCommand::PluginsList { state_filter, .. }) = parsed_list
    else {
        panic!("unexpected list parse result");
    };
    assert_eq!(state_filter, Some(PluginStateFilter::Disabled));

    let reset_args = vec![
        OsString::from("ah"),
        OsString::from("plugins"),
        OsString::from("reset"),
        OsString::from("--all"),
    ];
    let parsed_reset =
        parse_runtime_command(reset_args, &plugins).expect("reset --all should parse");
    let CliParseResult::Command(RuntimeCommand::PluginsReset { all, domain, .. }) = parsed_reset
    else {
        panic!("unexpected reset parse result");
    };
    assert!(all);
    assert_eq!(domain, None);
}
