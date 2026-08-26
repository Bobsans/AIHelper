//! What an invocation and a discovery pass report back.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunCheckOutcome {
    pub success: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationOutcome {
    RunCheck(RunCheckOutcome),
}

#[derive(Debug, Clone)]
pub struct InvocationObservation {
    pub response: InvocationResponse,
    pub outcome: Option<InvocationOutcome>,
}

impl InvocationObservation {
    pub fn new(response: InvocationResponse, outcome: InvocationOutcome) -> Self {
        Self {
            response,
            outcome: Some(outcome),
        }
    }

    pub fn without_outcome(response: InvocationResponse) -> Self {
        Self {
            response,
            outcome: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSource {
    Builtin,
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct PluginLoadWarning {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct PluginLoadConflict {
    pub domain: String,
    pub winner: PluginMetadata,
    pub loser: PluginMetadata,
    pub winner_source: PluginSource,
    pub loser_source: PluginSource,
    pub reason: String,
}

#[derive(Debug, Default, Clone)]
pub struct PluginLoadReport {
    pub loaded: usize,
    pub skipped: usize,
    pub warnings: Vec<PluginLoadWarning>,
    pub conflicts: Vec<PluginLoadConflict>,
}

impl PluginLoadReport {
    pub(super) fn push_warning(&mut self, path: PathBuf, error: impl Into<String>) {
        self.skipped += 1;
        self.warnings.push(PluginLoadWarning {
            path,
            error: error.into(),
        });
    }

    pub(super) fn push_conflict(
        &mut self,
        winner: PluginMetadata,
        loser: PluginMetadata,
        winner_source: PluginSource,
        loser_source: PluginSource,
        reason: impl Into<String>,
    ) {
        let domain = winner.domain.clone();
        self.conflicts.push(PluginLoadConflict {
            domain,
            winner,
            loser,
            winner_source,
            loser_source,
            reason: reason.into(),
        });
    }
}
