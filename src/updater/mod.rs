pub(crate) mod check;
// Candidate preparation is wired into the upgrade command in the next roadmap stage.
#[allow(dead_code)]
pub(crate) mod candidate;
pub(crate) mod command;
pub(crate) mod github;
mod trust;

pub(crate) use check::execute;
