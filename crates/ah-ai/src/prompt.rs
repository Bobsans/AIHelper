use std::io::IsTerminal;

use dialoguer::{Confirm, Input, MultiSelect, Select};

use ah_error::AppError;

use super::{
    managed::{self, ManagedState, Snapshot},
    targets::{Scope, Target, Transport},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportChoice {
    Managed,
    Http,
    Stdio,
}

#[derive(Debug, Clone)]
pub struct Answers {
    pub scope: Scope,
    pub with_mcp: bool,
    pub with_rules: bool,
    pub transport: Transport,
    pub managed: bool,
    pub url: Option<String>,
}

/// Prompts run only on a terminal and only when no decision was already made on
/// the command line, so scripts and pipes always take the documented defaults.
pub fn wants_prompts(has_decision_flags: bool) -> bool {
    !has_decision_flags && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

fn failed(what: &str) -> AppError {
    AppError::external(
        "AI_PROMPT_FAILED",
        format!("failed to read the {what} selection"),
    )
}

/// The transport menu, filtered by platform. `managed` is `None` when the
/// managed service is unavailable or could not be inspected.
pub fn transport_options(managed: Option<&Snapshot>) -> Vec<(String, TransportChoice)> {
    let mut options = Vec::new();
    if let Some(snapshot) = managed
        && snapshot.state != ManagedState::NeedsRepair
    {
        options.push((snapshot.label(), TransportChoice::Managed));
    }
    options.push(("HTTP endpoint (manual)".to_owned(), TransportChoice::Http));
    options.push((
        "stdio (agent spawns `ah mcp serve`)".to_owned(),
        TransportChoice::Stdio,
    ));
    options
}

pub fn ask(target: &Target, default_url: &str) -> Result<Answers, AppError> {
    let scope = ask_scope(target)?;
    let (with_mcp, with_rules) = ask_components()?;

    if !with_mcp && !with_rules {
        return Err(AppError::external(
            "AI_NOTHING_SELECTED",
            "neither the MCP server nor the rules block was selected; nothing to install",
        ));
    }

    if !with_mcp {
        return Ok(Answers {
            scope,
            with_mcp,
            with_rules,
            transport: Transport::Stdio,
            managed: false,
            url: None,
        });
    }

    let snapshot = if managed::is_supported() {
        managed::detect().ok()
    } else {
        None
    };
    let options = transport_options(snapshot.as_ref());
    let labels = options
        .iter()
        .map(|(label, _)| label.as_str())
        .collect::<Vec<_>>();
    let index = Select::new()
        .with_prompt("MCP transport")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|_| failed("transport"))?;

    let (transport, managed, url) = match options[index].1 {
        TransportChoice::Managed => (Transport::Http, true, None),
        TransportChoice::Stdio => (Transport::Stdio, false, None),
        TransportChoice::Http => {
            let url: String = Input::new()
                .with_prompt("MCP endpoint")
                .default(default_url.to_owned())
                .interact_text()
                .map_err(|_| failed("endpoint"))?;
            (Transport::Http, false, Some(url))
        }
    };

    Ok(Answers {
        scope,
        with_mcp,
        with_rules,
        transport,
        managed,
        url,
    })
}

fn ask_scope(target: &Target) -> Result<Scope, AppError> {
    if target.scopes.len() == 1 {
        return Ok(target.scopes[0]);
    }
    let labels = target
        .scopes
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>();
    let default = target
        .scopes
        .iter()
        .position(|scope| *scope == target.default_scope)
        .unwrap_or(0);
    let index = Select::new()
        .with_prompt("Configuration scope")
        .items(&labels)
        .default(default)
        .interact()
        .map_err(|_| failed("scope"))?;
    Ok(target.scopes[index])
}

fn ask_components() -> Result<(bool, bool), AppError> {
    let selected = MultiSelect::new()
        .with_prompt("Install")
        .items(["MCP server", "rules block"])
        .defaults(&[true, true])
        .interact()
        .map_err(|_| failed("component"))?;
    Ok((selected.contains(&0), selected.contains(&1)))
}

/// Nothing is executed or written before this returns true.
pub fn confirm(summary: &str) -> Result<bool, AppError> {
    eprintln!("{summary}");
    Confirm::new()
        .with_prompt("Apply")
        .default(true)
        .interact()
        .map_err(|_| failed("confirmation"))
}

#[cfg(test)]
mod tests {
    use super::{TransportChoice, transport_options};
    use crate::managed::{ManagedState, Snapshot};

    fn snapshot(state: ManagedState) -> Snapshot {
        Snapshot {
            state,
            endpoint: Some("http://127.0.0.1:8787/mcp".to_owned()),
            diagnostic_code: None,
        }
    }

    #[test]
    fn managed_leads_the_menu_when_it_is_usable() {
        let options = transport_options(Some(&snapshot(ManagedState::Running)));
        assert_eq!(
            options
                .iter()
                .map(|(_, choice)| *choice)
                .collect::<Vec<_>>(),
            vec![
                TransportChoice::Managed,
                TransportChoice::Http,
                TransportChoice::Stdio
            ]
        );
        assert!(options[0].0.contains("running"));
    }

    #[test]
    fn a_stopped_service_is_offered_with_its_state_in_the_label() {
        let options = transport_options(Some(&snapshot(ManagedState::Stopped)));
        assert_eq!(options[0].1, TransportChoice::Managed);
        assert!(options[0].0.contains("will be started"));
    }

    #[test]
    fn an_unavailable_or_broken_service_is_absent_from_the_menu() {
        for snapshot in [None, Some(&snapshot(ManagedState::NeedsRepair))] {
            let options = transport_options(snapshot);
            assert_eq!(
                options
                    .iter()
                    .map(|(_, choice)| *choice)
                    .collect::<Vec<_>>(),
                vec![TransportChoice::Http, TransportChoice::Stdio],
                "a service that cannot be used must not be selectable"
            );
        }
    }
}
