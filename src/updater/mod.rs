pub(crate) mod check;
// Candidate preparation is wired into the upgrade command in the next roadmap stage.
#[allow(dead_code)]
pub(crate) mod candidate;
pub(crate) mod command;
pub(crate) mod github;
#[allow(dead_code)]
mod installation;
#[allow(dead_code)]
mod smoke;
mod trust;

pub(crate) use check::execute;
