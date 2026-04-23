use ah_plugin_api::PluginManual;
use ah_runtime::PluginManager;
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

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

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => render_text(domain_filter, &host_commands, &manuals),
        OutputMode::Json => render_json(domain_filter, host_commands, manuals)?,
    }

    Ok(())
}

fn render_text(
    domain_filter: Option<&str>,
    host_commands: &[HostCommandDoc],
    manuals: &[PluginManual],
) {
    println!("AIHelper agent manual");
    println!("usage: ah <domain> <command> [options]");
    if let Some(filter) = domain_filter {
        println!("domain filter: {filter}");
    }
    println!();

    println!("Global flags:");
    for option in global_options_docs() {
        println!("  {}  {}", option.flag, option.description);
    }
    println!();

    println!("Host commands:");
    for command in host_commands {
        println!("  ah {}", command.usage);
        println!("    {}", command.summary);
        for example in &command.examples {
            println!("    e.g. {}: {}", example.description, example.command);
        }
    }
    println!();

    for manual in manuals {
        println!("Domain: {} ({})", manual.domain, manual.plugin_name);
        println!("  {}", manual.description);
        if !manual.notes.is_empty() {
            println!("  Notes:");
            for note in &manual.notes {
                println!("    - {}", note);
            }
        }
        println!("  Commands:");
        for command in &manual.commands {
            println!("    ah {} {}", manual.domain, command.usage);
            println!("      {}", command.summary);
            for example in &command.examples {
                let rendered = render_plugin_example(&manual.domain, &example.argv);
                println!("      e.g. {}: {}", example.description, rendered);
            }
        }
        println!();
    }
}

fn render_json(
    domain_filter: Option<&str>,
    host_commands: Vec<HostCommandDoc>,
    manuals: Vec<PluginManual>,
) -> Result<(), AppError> {
    let payload = AiInfoOutput {
        command: "ai.info",
        domain_filter: domain_filter.map(str::to_owned),
        global_options: global_options_docs(),
        host_commands,
        plugin_count: manuals.len(),
        plugins: manuals,
    };
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
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
            name: "plugins.list".to_owned(),
            summary: "List registered plugins and their metadata.".to_owned(),
            usage: "plugins list [--json]".to_owned(),
            examples: vec![HostCommandExample {
                description: "Inspect plugin registry".to_owned(),
                command: "ah plugins list --json".to_owned(),
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

#[derive(Debug, Serialize)]
struct AiInfoOutput {
    command: &'static str,
    domain_filter: Option<String>,
    global_options: Vec<GlobalOptionDoc>,
    host_commands: Vec<HostCommandDoc>,
    plugin_count: usize,
    plugins: Vec<PluginManual>,
}

#[derive(Debug, Clone, Serialize)]
struct GlobalOptionDoc {
    flag: &'static str,
    description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct HostCommandDoc {
    name: String,
    summary: String,
    usage: String,
    examples: Vec<HostCommandExample>,
}

#[derive(Debug, Clone, Serialize)]
struct HostCommandExample {
    description: String,
    command: String,
}
