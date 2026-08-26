//! The built-in command domains.
//!
//! One module per domain, each in the same three layers: `domain` is pure,
//! `io` performs the effects, `output` renders. `layout.rs` checks that shape
//! rather than describing it, because prose in a contributing guide had already
//! failed to hold the line twice.
//!
//! The domains know nothing of the CLI. They take arguments, return results, and
//! render through `ah-output`; which of them exists, and how each is reached
//! from the command line, is `ah`'s business and is declared there.

pub mod ctx;
pub mod file;
pub mod git;
pub mod git_status;
pub mod http;
mod layout;
pub mod project;
pub mod run;
pub mod safety;
pub mod search;
pub mod secrets;
pub mod task;
