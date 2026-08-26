//! What the run tells the event log, and what it prints about plugin
//! discovery.

use super::*;

pub(super) fn successful_exit_command_name(argv: &[String]) -> &'static str {
    if argv
        .iter()
        .any(|argument| matches!(argument.as_str(), "--version" | "-V"))
    {
        "version"
    } else {
        "help"
    }
}

pub(super) fn render_discovery_diagnostics(
    load_report: &mut PluginLoadReport,
    command: &RuntimeCommand,
) {
    if crate::command_is_quiet(command) {
        return;
    }

    load_report
        .conflicts
        .sort_by(|left, right| left.domain.cmp(&right.domain));
    load_report
        .warnings
        .sort_by(|left, right| left.path.cmp(&right.path));
    for warning in &load_report.warnings {
        emit_warning(format!(
            "skipped plugin {}: {}",
            warning.path.display(),
            warning.error
        ));
    }
    for conflict in &load_report.conflicts {
        emit_warning(format!(
            "domain '{}' conflict: {}",
            conflict.domain, conflict.reason
        ));
        emit_muted_stderr(format!(
            "  keeping {} plugin '{}', ignored {} plugin '{}'",
            plugin_source_name(conflict.winner_source),
            conflict.winner.plugin_name,
            plugin_source_name(conflict.loser_source),
            conflict.loser.plugin_name
        ));
    }
}

pub(super) fn record_discovery_events(
    logger: Option<&EventLogger>,
    load_report: &PluginLoadReport,
) {
    let Some(logger) = logger else {
        return;
    };
    for warning in &load_report.warnings {
        logger.record_system_event(
            "plugin_discovery",
            SystemEventSeverity::Warning,
            EventDiagnostic::new(
                "PLUGIN_LOAD_WARNING",
                "dynamic plugin was skipped during discovery",
                0,
            )
            .with_cause(warning.error.clone()),
            serde_json::json!({"path": warning.path.to_string_lossy()}),
        );
    }
    for conflict in &load_report.conflicts {
        logger.record_system_event(
            "plugin_discovery",
            SystemEventSeverity::Warning,
            EventDiagnostic::new(
                "PLUGIN_DOMAIN_CONFLICT",
                "plugin domain conflict was resolved",
                0,
            )
            .with_identity(Some(conflict.domain.clone()), None)
            .with_cause(conflict.reason.clone()),
            serde_json::json!({
                "winner": conflict.winner.plugin_name,
                "winner_source": plugin_source_name(conflict.winner_source),
                "loser": conflict.loser.plugin_name,
                "loser_source": plugin_source_name(conflict.loser_source),
            }),
        );
    }
}

pub(super) fn record_app_system_error(
    logger: Option<&EventLogger>,
    component: &str,
    error: &AppError,
    context: serde_json::Value,
) {
    if let Some(logger) = logger {
        logger.record_system_event(
            component,
            SystemEventSeverity::Error,
            EventDiagnostic::from_app_error(error),
            context,
        );
    }
}
