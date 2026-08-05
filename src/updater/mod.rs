pub(crate) mod check;
// Candidate preparation is wired into the upgrade command in the next roadmap stage.
#[allow(dead_code)]
pub(crate) mod candidate;
pub(crate) mod command;
pub(crate) mod github;
#[cfg(windows)]
mod handoff;
#[allow(dead_code)]
mod installation;
pub(crate) mod recovery;
#[allow(dead_code)]
mod smoke;
mod trust;

pub(crate) use check::execute;
