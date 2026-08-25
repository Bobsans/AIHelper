pub mod install;
mod json_config;
pub mod managed;
mod opencode_config;
mod prompt;

/// Whether `ah ai install` should prompt: a terminal invocation that carried no
/// decision flags.
pub fn prompt_wanted(has_decision_flags: bool) -> bool {
    prompt::wants_prompts(has_decision_flags)
}
mod registrar;
mod rules;
pub mod targets;

use ah_plugin_api::PluginManual;
use ah_runtime::PluginManager;
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{Emitter, TextFormatter, TextStyle},
};

pub fn execute_info(
    manager: &PluginManager,
    domain_filter: Option<&str>,
    options: GlobalOptions,
) -> Result<(), AppError> {
    let host_commands = host_command_docs();
    let mut manuals = manager.collect_plugin_manuals();

    if let Some(filter) = domain_filter {
        manuals.retain(|manual| manual.domain.eq_ignore_ascii_case(filter));
        if manuals.is_empty() {
            return Err(AppError::invalid_argument(format!(
                "unknown domain for ai info: {filter}"
            )));
        }
    }

    let payload = AiInfoOutput {
        command: "ai.info",
        domain_filter: domain_filter.map(str::to_owned),
        global_options: global_options_docs(),
        plugin_count: manuals.len(),
        host_commands,
        plugins: manuals,
    };

    Emitter::stdio(&options).value(&payload, |formatter| {
        render_text(
            payload.domain_filter.as_deref(),
            &payload.host_commands,
            &payload.plugins,
            formatter,
        )
    })
}

pub(crate) fn typed_info_value(
    manager: &PluginManager,
    domain_filter: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let mut manuals = manager.collect_plugin_manuals();
    if let Some(filter) = domain_filter {
        manuals.retain(|manual| manual.domain.eq_ignore_ascii_case(filter));
        if manuals.is_empty() {
            return Err(AppError::invalid_argument(format!(
                "unknown domain for ai info: {filter}"
            )));
        }
    }
    let payload = AiInfoOutput {
        command: "ai.info",
        domain_filter: domain_filter.map(str::to_owned),
        global_options: global_options_docs(),
        host_commands: host_command_docs(),
        plugin_count: manuals.len(),
        plugins: manuals,
    };
    Ok(serde_json::to_value(payload)?)
}

fn render_text(
    domain_filter: Option<&str>,
    host_commands: &[HostCommandDoc],
    manuals: &[PluginManual],
    formatter: TextFormatter,
) -> String {
    let mut lines = vec![
        formatter.paint(TextStyle::Heading, "AIHelper agent manual"),
        formatter.paint(TextStyle::Key, "usage: ah <domain> <command> [options]"),
    ];
    if let Some(filter) = domain_filter {
        lines.push(format!(
            "{} {}",
            formatter.paint(TextStyle::Heading, "domain filter:"),
            formatter.paint(TextStyle::Key, filter)
        ));
    }
    lines.push(String::new());

    lines.push(formatter.paint(TextStyle::Heading, "Global flags:"));
    for option in global_options_docs() {
        lines.push(format!(
            "  {}  {}",
            formatter.paint(TextStyle::Key, option.flag),
            option.description
        ));
    }
    lines.push(String::new());

    lines.push(formatter.paint(TextStyle::Heading, "Host commands:"));
    for command in host_commands {
        lines.push(format!(
            "  {}",
            formatter.paint(TextStyle::Key, format!("ah {}", command.usage))
        ));
        lines.push(format!("    {}", command.summary));
        for example in &command.examples {
            lines.push(format!(
                "    {} {}: {}",
                formatter.paint(TextStyle::Muted, "e.g."),
                example.description,
                formatter.paint(TextStyle::Key, &example.command)
            ));
        }
    }
    lines.push(String::new());

    for manual in manuals {
        lines.push(format!(
            "{} {} ({})",
            formatter.paint(TextStyle::Heading, "Domain:"),
            formatter.paint(TextStyle::Key, &manual.domain),
            formatter.paint(TextStyle::Muted, &manual.plugin_name)
        ));
        lines.push(format!("  {}", manual.description));
        if !manual.notes.is_empty() {
            lines.push(format!(
                "  {}",
                formatter.paint(TextStyle::Heading, "Notes:")
            ));
            lines.extend(manual.notes.iter().map(|note| format!("    - {note}")));
        }
        lines.push(format!(
            "  {}",
            formatter.paint(TextStyle::Heading, "Commands:")
        ));
        for command in &manual.commands {
            lines.push(format!(
                "    {}",
                formatter.paint(
                    TextStyle::Key,
                    format!("ah {} {}", manual.domain, command.usage)
                )
            ));
            lines.push(format!("      {}", command.summary));
            for example in &command.examples {
                let rendered = render_plugin_example(&manual.domain, &example.argv);
                lines.push(format!(
                    "      {} {}: {}",
                    formatter.paint(TextStyle::Muted, "e.g."),
                    example.description,
                    formatter.paint(TextStyle::Key, rendered)
                ));
            }
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

fn render_plugin_example(domain: &str, argv: &[String]) -> String {
    let mut rendered = String::from("ah ");
    rendered.push_str(domain);
    if !argv.is_empty() {
        rendered.push(' ');
        rendered.push_str(&argv.join(" "));
    }
    rendered
}

fn host_command_docs() -> Vec<HostCommandDoc> {
    vec![
        HostCommandDoc {
            name: "ai.info".to_owned(),
            summary: "Show AI-agent manual for all domains or one selected domain.".to_owned(),
            usage: "ai info [--domain DOMAIN]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Show complete manual".to_owned(),
                    command: "ah ai info".to_owned(),
                },
                HostCommandExample {
                    description: "Show manual only for search domain".to_owned(),
                    command: "ah ai info --domain search".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "ai.install".to_owned(),
            summary: "Register the `aihelper` MCP server and rules block in an AI coding agent (claude, codex, gemini, cursor, copilot, opencode)."
                .to_owned(),
            usage: "ai install <TARGET> [--scope <local|project|user>] [--transport <stdio|http|managed>] [--url URL] [--mcp-only|--rules-only] [--yes] [--dry-run]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Wire AIHelper into Claude Code for this project".to_owned(),
                    command: "ah ai install claude".to_owned(),
                },
                HostCommandExample {
                    description: "Point Codex at the local HTTP MCP endpoint".to_owned(),
                    command: "ah ai install codex --transport http".to_owned(),
                },
                HostCommandExample {
                    description: "Use the Windows managed service, installing it if absent"
                        .to_owned(),
                    command: "ah ai install cursor --transport managed".to_owned(),
                },
                HostCommandExample {
                    description: "Preview the commands and file changes only".to_owned(),
                    command: "ah ai install claude --dry-run".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "ai.uninstall".to_owned(),
            summary: "Remove the AIHelper MCP server, including a legacy `ah` registration, and the rules block from an AI coding agent."
                .to_owned(),
            usage: "ai uninstall <TARGET> [--scope <local|project|user>] [--dry-run]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Remove the AIHelper integration from Codex".to_owned(),
                command: "ah ai uninstall codex".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "ai.status".to_owned(),
            summary: "Report AIHelper integration state for known AI coding agents.".to_owned(),
            usage: "ai status [TARGET]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Show integration state for every known agent".to_owned(),
                    command: "ah ai status".to_owned(),
                },
                HostCommandExample {
                    description: "Show integration state for one agent as JSON".to_owned(),
                    command: "ah --json ai status claude".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "mcp.serve".to_owned(),
            summary: "Serve typed AIHelper tools over MCP stdio or local HTTP.".to_owned(),
            usage: "mcp serve [--transport <stdio|http>] [--port PORT] [--max-active N] [--default-timeout-ms MILLISECONDS]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Start the stdio MCP server".to_owned(),
                    command: "ah mcp serve".to_owned(),
                },
                HostCommandExample {
                    description: "Start the local Streamable HTTP MCP server".to_owned(),
                    command: "ah mcp serve --transport http --port 8787".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "mcp.service.install".to_owned(),
            summary: "Install or reconcile the per-user Windows MCP service and wait for readiness by default.".to_owned(),
            usage: "mcp service install [--no-start] [--port PORT] [--max-active N] [--default-timeout-ms MILLISECONDS]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Install and start the managed service".to_owned(),
                    command: "ah mcp service install --json".to_owned(),
                },
                HostCommandExample {
                    description: "Reconcile registration without changing the process".to_owned(),
                    command: "ah mcp service install --no-start --json".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "mcp.service.start".to_owned(),
            summary: "Start the registered Windows MCP service and require exact readiness."
                .to_owned(),
            usage: "mcp service start [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Start or reuse the exact managed instance".to_owned(),
                command: "ah mcp service start --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "mcp.service.stop".to_owned(),
            summary: "Stop only the exact managed instance, with a bounded Task Scheduler fallback."
                .to_owned(),
            usage: "mcp service stop [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Stop the installed managed service safely".to_owned(),
                command: "ah mcp service stop --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "mcp.service.restart".to_owned(),
            summary: "Stop and start the managed service under one lifecycle operation, requiring a new instance identity."
                .to_owned(),
            usage: "mcp service restart [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Restart and wait for a new exact instance".to_owned(),
                command: "ah mcp service restart --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "mcp.service.status".to_owned(),
            summary: "Read registration, scheduler, runtime, readiness, lifecycle, and drift state without mutation.".to_owned(),
            usage: "mcp service status [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Inspect the complete managed lifecycle snapshot".to_owned(),
                command: "ah mcp service status --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "mcp.service.uninstall".to_owned(),
            summary: "Stop and remove the owned managed registration and verified lifecycle metadata."
                .to_owned(),
            usage: "mcp service uninstall [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Uninstall without deleting binaries, plugins, configuration, or logs"
                    .to_owned(),
                command: "ah mcp service uninstall --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "plugins.list".to_owned(),
            summary: "List registered plugins and their metadata.".to_owned(),
            usage: "plugins list [--state <enabled|disabled>] [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Inspect plugin registry".to_owned(),
                command: "ah plugins list --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "plugins.enable".to_owned(),
            summary: "Enable a disabled plugin domain.".to_owned(),
            usage: "plugins enable <domain> [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Enable HTTP domain".to_owned(),
                command: "ah plugins enable http".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "plugins.disable".to_owned(),
            summary: "Disable plugin domain (built-in or dynamic).".to_owned(),
            usage: "plugins disable <domain> [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Disable ollama domain".to_owned(),
                command: "ah plugins disable ollama".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "plugins.reset".to_owned(),
            summary: "Reset plugin domain override(s) back to default state.".to_owned(),
            usage: "plugins reset <domain> [--json] | plugins reset --all [--json]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Reset one domain override".to_owned(),
                    command: "ah plugins reset http".to_owned(),
                },
                HostCommandExample {
                    description: "Reset all overrides".to_owned(),
                    command: "ah plugins reset --all".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "secrets.init".to_owned(),
            summary: "Initialize the encrypted local secret vault.".to_owned(),
            usage: "secrets init".to_owned(),
            examples: vec![HostCommandExample {
                description: "Initialize the vault before adding secrets".to_owned(),
                command: "ah secrets init".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "secrets.list".to_owned(),
            summary: "List redacted secret metadata. Agents should discover secret IDs through the typed/MCP secrets.list command and may filter by kind."
                .to_owned(),
            usage: "secrets list [--kind <postgres|http-basic|ssh-key>]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Discover PostgreSQL secret IDs without reading values".to_owned(),
                command: "ah secrets list --kind postgres --json".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "secrets.add".to_owned(),
            summary: "Add a secret through hidden terminal prompts; values are never accepted through argv."
                .to_owned(),
            usage: "secrets add <id> --kind <postgres|http-basic|ssh-key> [--label TEXT] [--description TEXT] [--open]".to_owned(),
            examples: vec![
                HostCommandExample {
                    description: "Add PostgreSQL credentials using a hidden password prompt".to_owned(),
                    command: "ah secrets add billing --kind postgres --label Billing".to_owned(),
                },
                HostCommandExample {
                    description: "Open a protected form on the local HTTP MCP server".to_owned(),
                    command: "ah secrets add billing --kind postgres --open".to_owned(),
                },
            ],
        },
        HostCommandDoc {
            name: "secrets.edit".to_owned(),
            summary: "Edit secret metadata and values through hidden terminal prompts.".to_owned(),
            usage: "secrets edit <id> [--label TEXT] [--description TEXT] [--open]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Edit a secret without exposing values in argv".to_owned(),
                command: "ah secrets edit billing --label Billing".to_owned(),
            }],
        },
        HostCommandDoc {
            name: "secrets.remove".to_owned(),
            summary: "Remove one secret from the encrypted vault.".to_owned(),
            usage: "secrets remove <id>".to_owned(),
            examples: vec![HostCommandExample {
                description: "Remove a stored secret".to_owned(),
                command: "ah secrets remove billing".to_owned(),
            }],
        },
    ]
}

fn global_options_docs() -> Vec<GlobalOptionDoc> {
    vec![
        GlobalOptionDoc {
            flag: "--json",
            description: "Return machine-readable JSON output",
        },
        GlobalOptionDoc {
            flag: "--quiet",
            description: "Suppress command output",
        },
        GlobalOptionDoc {
            flag: "--cwd <PATH>",
            description: "Execute command with explicit working directory",
        },
        GlobalOptionDoc {
            flag: "--limit <N>",
            description: "Cap output lines/items when command supports limits",
        },
    ]
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AiInfoOutput {
    command: &'static str,
    domain_filter: Option<String>,
    global_options: Vec<GlobalOptionDoc>,
    host_commands: Vec<HostCommandDoc>,
    plugin_count: usize,
    plugins: Vec<PluginManual>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GlobalOptionDoc {
    flag: &'static str,
    description: &'static str,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HostCommandDoc {
    name: String,
    summary: String,
    usage: String,
    examples: Vec<HostCommandExample>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HostCommandExample {
    description: String,
    command: String,
}
