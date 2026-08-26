//! The `ah secrets` command surface.

mod domain;
mod io;
mod output;

pub use domain::{SecretsCommand, execute};
