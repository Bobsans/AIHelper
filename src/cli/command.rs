//! The clap command tree, built once the plugin catalog is known.
//!
//! Declaration only: what flags exist and what they are called. Reading a
//! parse out of them is `parse`.

use super::*;

pub(super) const HOST_COMMAND_AI: &str = "ai";

pub(super) const HOST_COMMAND_MCP: &str = "mcp";

pub(super) const HOST_COMMAND_PLUGINS: &str = "plugins";

pub(super) const HOST_COMMAND_SECRETS: &str = "secrets";

pub(super) const HOST_COMMAND_UPGRADE: &str = "upgrade";

pub(crate) fn build_cli_command(plugins: &[PluginMetadata]) -> Command {
    let mut command = crate::entry::with_global_flags(
        Command::new("ah")
            .version(env!("CARGO_PKG_VERSION"))
            .about("AIHelper CLI toolbox for AI agents and developers"),
    )
    .subcommand(build_ai_command())
    .subcommand(build_mcp_command())
    .subcommand(build_plugins_command())
    .subcommand(build_secrets_command())
    .subcommand(crate::upgrade::route::build_help_command())
    .allow_external_subcommands(true);

    for (domain, description) in plugin_domains_for_help(plugins) {
        if domain == HOST_COMMAND_AI
            || domain == HOST_COMMAND_MCP
            || domain == HOST_COMMAND_PLUGINS
            || domain == HOST_COMMAND_SECRETS
            || domain == HOST_COMMAND_UPGRADE
        {
            continue;
        }
        command = command.subcommand(build_domain_command(&domain, &description));
    }

    command
}

pub(super) fn build_mcp_command() -> Command {
    Command::new(HOST_COMMAND_MCP)
        .about("Model Context Protocol server")
        .subcommand(
            Command::new("serve")
                .about("Serve typed AIHelper tools over MCP stdio or local HTTP")
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .value_name("TRANSPORT")
                        .value_parser(["stdio", "http"])
                        .default_value("stdio")
                        .help("MCP transport: stdio or local Streamable HTTP"),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .value_parser(value_parser!(u16))
                        .help("Local HTTP port (default: 8787; HTTP transport only)"),
                )
                .arg(
                    Arg::new("max-active")
                        .long("max-active")
                        .value_name("N")
                        .value_parser(value_parser!(usize))
                        .default_value("32")
                        .help("Maximum number of concurrently active command handlers"),
                )
                .arg(
                    Arg::new("max-queued")
                        .long("max-queued")
                        .value_name("N")
                        .value_parser(value_parser!(usize))
                        .hide(true),
                )
                .arg(
                    Arg::new("default-timeout-ms")
                        .long("default-timeout-ms")
                        .value_name("MILLISECONDS")
                        .value_parser(value_parser!(u64))
                        .default_value("300000")
                        .help("Default command execution timeout"),
                )
                .arg(crate::mcp_service::command::managed_config_arg()),
        )
        .subcommand(crate::mcp_service::command::build_service_help_command())
}

pub(super) fn build_ai_command() -> Command {
    Command::new(HOST_COMMAND_AI)
        .about("AI-agent manual and agent integration")
        .subcommand(
            Command::new("info")
                .about("Show full AI-agent manual for available commands")
                .arg(
                    Arg::new("domain")
                        .long("domain")
                        .value_name("DOMAIN")
                        .value_parser(value_parser!(String))
                        .help("Show manual only for a single command domain"),
                ),
        )
        .subcommand(
            Command::new("install")
                .about("Register the AIHelper MCP server and rules block in an AI agent")
                .arg(target_arg(true))
                .arg(scope_arg())
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .value_name("TRANSPORT")
                        .value_parser(["stdio", "http", "managed"])
                        .default_value("stdio")
                        .help(
                            "MCP transport written into the agent configuration;                              `managed` uses the Windows managed service endpoint",
                        ),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .value_name("URL")
                        .value_parser(value_parser!(String))
                        .help("Loopback MCP endpoint; only valid with --transport http"),
                )
                .arg(
                    Arg::new("mcp-only")
                        .long("mcp-only")
                        .action(ArgAction::SetTrue)
                        .help("Register the MCP server without touching the rules file"),
                )
                .arg(
                    Arg::new("rules-only")
                        .long("rules-only")
                        .action(ArgAction::SetTrue)
                        .help("Install the rules block without registering the MCP server"),
                )
                .group(ArgGroup::new("ai-components").args(["mcp-only", "rules-only"]))
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .short('y')
                        .action(ArgAction::SetTrue)
                        .help("Skip the interactive confirmation prompt"),
                )
                .arg(dry_run_arg()),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove the AIHelper MCP server and rules block from an AI agent")
                .arg(target_arg(true))
                .arg(scope_arg())
                .arg(dry_run_arg()),
        )
        .subcommand(
            Command::new("status")
                .about("Report AIHelper integration state for known AI agents")
                .arg(target_arg(false)),
        )
}

pub(super) fn target_arg(required: bool) -> Arg {
    Arg::new("target")
        .value_name("TARGET")
        .required(required)
        .value_parser(value_parser!(String))
        .help("Agent target, for example claude or codex")
}

pub(super) fn scope_arg() -> Arg {
    Arg::new("scope")
        .long("scope")
        .value_name("SCOPE")
        .value_parser(["local", "project", "user"])
        .help("Configuration scope understood by the target agent")
}

pub(super) fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Report planned commands and file changes without performing them")
}

pub(super) fn build_plugins_command() -> Command {
    Command::new(HOST_COMMAND_PLUGINS)
        .about("Plugin management commands")
        .subcommand(
            Command::new("list").about("List registered plugins").arg(
                Arg::new("state")
                    .long("state")
                    .value_name("STATE")
                    .value_parser(["enabled", "disabled"])
                    .help("Filter by plugin domain state"),
            ),
        )
        .subcommand(
            Command::new("enable")
                .about("Enable plugin domain")
                .arg(Arg::new("domain").value_name("DOMAIN").required(true)),
        )
        .subcommand(
            Command::new("disable")
                .about("Disable plugin domain")
                .arg(Arg::new("domain").value_name("DOMAIN").required(true)),
        )
        .subcommand(
            Command::new("reset")
                .about("Reset plugin domain override")
                .arg(Arg::new("domain").value_name("DOMAIN").required(false))
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Reset all domain overrides"),
                )
                .group(
                    ArgGroup::new("plugins-reset-target")
                        .args(["domain", "all"])
                        .required(true)
                        .multiple(false),
                ),
        )
}

pub(super) fn build_secrets_command() -> Command {
    let metadata_args = || {
        [
            Arg::new("label")
                .long("label")
                .value_name("TEXT")
                .help("Human-readable secret label"),
            Arg::new("description")
                .long("description")
                .value_name("TEXT")
                .help("Optional secret description"),
        ]
    };
    Command::new(HOST_COMMAND_SECRETS)
        .about("Encrypted secret vault management")
        .subcommand(Command::new("init").about("Initialize the encrypted secret vault"))
        .subcommand(
            Command::new("list")
                .about("List redacted secret metadata")
                .arg(
                    Arg::new("kind")
                        .long("kind")
                        .value_name("KIND")
                        .value_parser(secret_kind_values())
                        .help("Filter by secret kind"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a secret using hidden prompts")
                .arg(Arg::new("id").value_name("ID").required(true))
                .arg(
                    Arg::new("kind")
                        .long("kind")
                        .value_name("KIND")
                        .value_parser(secret_kind_values())
                        .required(true),
                )
                .arg(
                    Arg::new("open")
                        .long("open")
                        .action(ArgAction::SetTrue)
                        .help("Open a protected browser setup form"),
                )
                .args(metadata_args()),
        )
        .subcommand(
            Command::new("edit")
                .about("Edit a secret using hidden prompts")
                .arg(Arg::new("id").value_name("ID").required(true))
                .arg(
                    Arg::new("open")
                        .long("open")
                        .action(ArgAction::SetTrue)
                        .help("Open a protected browser setup form"),
                )
                .args(metadata_args()),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove a secret")
                .arg(Arg::new("id").value_name("ID").required(true)),
        )
}

pub(super) fn build_domain_command(domain: &str, description: &str) -> Command {
    Command::new(domain.to_owned())
        .about(description.to_owned())
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("argv")
                .num_args(0..)
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .trailing_var_arg(true)
                .value_parser(value_parser!(String)),
        )
}

pub(super) fn plugin_domains_for_help(plugins: &[PluginMetadata]) -> Vec<(String, String)> {
    let mut by_domain = BTreeMap::new();
    for plugin in plugins {
        by_domain.insert(
            plugin.domain.clone(),
            top_level_domain_summary(&plugin.domain, &plugin.description),
        );
    }
    by_domain.into_iter().collect()
}

pub(super) fn top_level_domain_summary(domain: &str, fallback: &str) -> String {
    match domain {
        "file" => "File utilities".to_owned(),
        "search" => "Search utilities".to_owned(),
        "ctx" => "Context-reduction utilities".to_owned(),
        "git" => "Git-focused utilities".to_owned(),
        "project" => "Project detection utilities".to_owned(),
        "run" => "Command execution check utilities".to_owned(),
        "http" => "HTTP workflow utilities".to_owned(),
        "task" => "Task recipe utilities".to_owned(),
        _ => fallback.to_owned(),
    }
}

pub(super) fn secret_kind_values() -> Vec<&'static str> {
    crate::secrets::SecretKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect()
}
