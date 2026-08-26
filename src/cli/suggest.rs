//! What to say when the command does not exist: the nearest name, and the
//! diagnostic that carries it.

use super::*;

pub(super) fn decorate_cli_parse_error(
    error: AppError,
    command: &Command,
    raw_args: &[OsString],
) -> AppError {
    let detail = error.detail_message();
    let Some(candidate) = suggested_subcommand(&detail) else {
        return error;
    };
    let mut scope = command;
    let mut index = 1usize;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        let Some(argument) = raw_args[index].to_str() else {
            break;
        };
        let Some(subcommand) = scope.find_subcommand(argument) else {
            break;
        };
        scope = subcommand;
        index += 1;
    }
    let description = scope
        .find_subcommand(candidate)
        .and_then(Command::get_about)
        .map(ToString::to_string);
    error.with_suggestion_context(description, None)
}

pub(crate) fn suggest_top_level_command(
    domain: &str,
    plugins: &[PluginMetadata],
) -> Option<CommandSuggestion> {
    let normalized = domain.to_ascii_lowercase();
    let aliases = [
        ("version", "ah --version", "Show the AIHelper version"),
        ("help", "ah --help", "Show command help"),
    ];
    let mut candidates = aliases
        .iter()
        .map(|(name, command, description)| {
            (
                (*name).to_owned(),
                (*command).to_owned(),
                (*description).to_owned(),
            )
        })
        .collect::<Vec<_>>();
    candidates.extend(
        [
            (HOST_COMMAND_AI, "AI-agent focused command manual"),
            (HOST_COMMAND_MCP, "Model Context Protocol server"),
            (HOST_COMMAND_PLUGINS, "Plugin management commands"),
            (HOST_COMMAND_SECRETS, "Encrypted secret vault management"),
            (
                HOST_COMMAND_UPGRADE,
                "Check for or install AIHelper updates",
            ),
        ]
        .into_iter()
        .map(|(name, description)| {
            (
                name.to_owned(),
                format!("ah {name}"),
                description.to_owned(),
            )
        }),
    );
    candidates.extend(plugins.iter().map(|plugin| {
        let name = plugin.domain.to_ascii_lowercase();
        (
            name.clone(),
            format!("ah {name}"),
            top_level_domain_summary(&name, &plugin.description),
        )
    }));
    candidates.sort();
    candidates.dedup_by(|left, right| left.0 == right.0);

    candidates
        .into_iter()
        .map(|(name, command, description)| {
            (
                edit_distance(&normalized, &name),
                name.len(),
                command,
                description,
            )
        })
        .filter(|(distance, candidate_len, _, _)| {
            *distance <= 2 && distance.saturating_mul(3) <= normalized.len().max(*candidate_len)
        })
        .min_by_key(|(distance, candidate_len, command, _)| {
            (
                *distance,
                normalized.len().abs_diff(*candidate_len),
                command.clone(),
            )
        })
        .map(|(_, _, command, description)| CommandSuggestion::new(command, Some(description)))
}

pub(super) fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(left_index + 1);
        for (right_index, right_char) in right.iter().enumerate() {
            let substitution = previous[right_index] + usize::from(left_char != *right_char);
            current.push(
                (current[right_index] + 1)
                    .min(previous[right_index + 1] + 1)
                    .min(substitution),
            );
        }
        previous = current;
    }
    previous[right.len()]
}
