//! What `ah postgres --help` and the plugin manual show.
//!
//! Prose and examples, kept apart from the code they describe because a
//! reworded example is not a behaviour change. Every example here is parsed by
//! a test, so a stale one fails the build.

use super::*;

pub(crate) fn plugin_manual() -> PluginManual {
    PluginManual {
        plugin_name: PLUGIN_NAME.to_owned(),
        domain: DOMAIN.to_owned(),
        description: DESCRIPTION.to_owned(),
        commands: vec![
            ManualCommand {
                name: "tool status".to_owned(),
                summary: "Show resolved PostgreSQL toolchain status.".to_owned(),
                usage: "tool status".to_owned(),
                examples: vec![ManualExample::new("Inspect selected psql", &["tool", "status"])],
            },
            ManualCommand {
                name: "tool download".to_owned(),
                summary: "Download a managed PostgreSQL toolchain.".to_owned(),
                usage: "tool download [--version VERSION] [--force]".to_owned(),
                examples: vec![ManualExample::new(
                    "Download PostgreSQL 18.4 tools",
                    &["tool", "download", "--version", "18.4"],
                )],
            },
            ManualCommand {
                name: "tool use".to_owned(),
                summary: "Persist an explicit PostgreSQL toolchain path.".to_owned(),
                usage: "tool use --path PATH".to_owned(),
                examples: vec![ManualExample::new(
                    "Use an unpacked PostgreSQL bin directory",
                    &["tool", "use", "--path", "C:\\PostgreSQL\\pgsql\\bin"],
                )],
            },
            ManualCommand {
                name: "tool cleanup".to_owned(),
                summary: "Remove managed PostgreSQL toolchain cache.".to_owned(),
                usage: "tool cleanup [--version VERSION]".to_owned(),
                examples: vec![ManualExample::new(
                    "Remove managed PostgreSQL 18.4 tools",
                    &["tool", "cleanup", "--version", "18.4"],
                )],
            },
            ManualCommand {
                name: "ping".to_owned(),
                summary: "Check PostgreSQL connection.".to_owned(),
                usage: "ping [connection flags] [--ensure-tool]".to_owned(),
                examples: vec![ManualExample::new(
                    "Check local database",
                    &["ping", "--database", "postgres", "--user", "postgres"],
                )],
            },
            ManualCommand {
                name: "info".to_owned(),
                summary: "Show server and session metadata.".to_owned(),
                usage: "info [connection flags]".to_owned(),
                examples: vec![ManualExample::new("Show connection info", &["info"])],
            },
            ManualCommand {
                name: "databases".to_owned(),
                summary: "List databases.".to_owned(),
                usage: "databases [connection flags]".to_owned(),
                examples: vec![ManualExample::new("List databases", &["databases"])],
            },
            ManualCommand {
                name: "schemas".to_owned(),
                summary: "List schemas.".to_owned(),
                usage: "schemas [--include-system]".to_owned(),
                examples: vec![ManualExample::new("List user schemas", &["schemas"])],
            },
            ManualCommand {
                name: "tables".to_owned(),
                summary: "List tables and table-like relations.".to_owned(),
                usage: "tables [--schema NAME] [--include-system]".to_owned(),
                examples: vec![ManualExample::new("List public tables", &["tables", "--schema", "public"])],
            },
            ManualCommand {
                name: "views".to_owned(),
                summary: "List views.".to_owned(),
                usage: "views [--schema NAME] [--include-system]".to_owned(),
                examples: vec![ManualExample::new("List public views", &["views", "--schema", "public"])],
            },
            ManualCommand {
                name: "describe".to_owned(),
                summary: "Describe a table, view, or materialized view.".to_owned(),
                usage: "describe <schema.object>".to_owned(),
                examples: vec![ManualExample::new("Describe a table", &["describe", "public.users"])],
            },
            ManualCommand {
                name: "indexes".to_owned(),
                summary: "List indexes.".to_owned(),
                usage: "indexes [--schema NAME] [--table NAME]".to_owned(),
                examples: vec![ManualExample::new(
                    "List table indexes",
                    &["indexes", "--schema", "public", "--table", "users"],
                )],
            },
            ManualCommand {
                name: "extensions".to_owned(),
                summary: "List installed or available extensions.".to_owned(),
                usage: "extensions [--available]".to_owned(),
                examples: vec![ManualExample::new("List installed extensions", &["extensions"])],
            },
            ManualCommand {
                name: "query".to_owned(),
                summary: "Run a read-only SQL query.".to_owned(),
                usage: "query --sql TEXT|--file PATH [--limit N]".to_owned(),
                examples: vec![ManualExample::new(
                    "Run read-only SQL",
                    &["query", "--sql", "select now() as current_time"],
                )],
            },
            ManualCommand {
                name: "exec".to_owned(),
                summary: "Execute explicit SQL mutations or admin commands.".to_owned(),
                usage: "exec --sql TEXT|--file PATH --yes [--single-transaction]".to_owned(),
                examples: vec![ManualExample::new(
                    "Run an explicit command",
                    &["exec", "--sql", "vacuum analyze", "--yes"],
                )],
            },
            ManualCommand {
                name: "explain".to_owned(),
                summary: "Explain a SQL query plan.".to_owned(),
                usage: "explain --sql TEXT|--file PATH [--analyze --yes] [--buffers]".to_owned(),
                examples: vec![ManualExample::new(
                    "Explain a query",
                    &["explain", "--sql", "select * from pg_class"],
                )],
            },
            ManualCommand {
                name: "activity".to_owned(),
                summary: "Show pg_stat_activity rows.".to_owned(),
                usage: "activity [--active] [--idle-in-tx] [--limit N]".to_owned(),
                examples: vec![ManualExample::new("List active sessions", &["activity", "--active"])],
            },
            ManualCommand {
                name: "locks".to_owned(),
                summary: "Show lock and blocking diagnostics.".to_owned(),
                usage: "locks [--blocking] [--limit N]".to_owned(),
                examples: vec![ManualExample::new("Show blocking locks", &["locks", "--blocking"])],
            },
            ManualCommand {
                name: "size".to_owned(),
                summary: "Show database, schema, or table sizes.".to_owned(),
                usage: "size [--schema NAME] [--table NAME]".to_owned(),
                examples: vec![ManualExample::new("Show current database size", &["size"])],
            },
            ManualCommand {
                name: "settings".to_owned(),
                summary: "Show PostgreSQL settings.".to_owned(),
                usage: "settings [--changed] [--limit N]".to_owned(),
                examples: vec![ManualExample::new("Show changed settings", &["settings", "--changed"])],
            },
        ],
        notes: vec![
            "Uses psql non-interactively with -X, ON_ERROR_STOP=1, and --no-password.".to_owned(),
            "Pass database passwords via --password-env, .pgpass, or libpq service files; never via command argv.".to_owned(),
            "Operational commands do not download tools unless --ensure-tool is provided.".to_owned(),
            "Use global --json for structured machine-readable output.".to_owned(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn manual_examples_parse() {
        let manual = plugin_manual();
        for command in &manual.commands {
            for example in &command.examples {
                let mut args = Vec::with_capacity(example.argv.len() + 1);
                args.push(manual.domain.clone());
                args.extend(example.argv.iter().cloned());
                let parse_result = PostgresCli::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for command '{}': argv={args:?}",
                    command.name
                );
            }
        }
    }
}
