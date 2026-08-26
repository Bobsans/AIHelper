//! `ah mcp service`: what this CLI adds around the managed service mechanism.
//!
//! The route reads the command line, `mcp_service::lifecycle` performs the
//! operation, and `render` prints what it reported. The mechanism used to print
//! for itself, which is why it knew `Emitter` and `GlobalOptions` existed.

pub(crate) mod render;
pub(crate) mod route;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    mcp_service::{
        lifecycle,
        operation::{Operation, Report},
    },
    output::Emitter,
};

/// Run one lifecycle operation and report it.
///
/// # Errors
///
/// [`AppError`] from the operation, or when the report cannot be written.
pub(crate) fn execute(operation: Operation, options: GlobalOptions) -> Result<(), AppError> {
    let report = lifecycle::run(operation)?;
    let mut emitter = Emitter::stdio(&options);
    match report {
        Report::Mutation(output) => render::mutation(&output, &mut emitter),
        Report::Status(output) => render::status(&output, &mut emitter),
        Report::Uninstall(output) => render::uninstall(&output, &mut emitter),
    }
}
