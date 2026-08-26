mod activate;
pub(crate) mod candidate;
pub(crate) mod check;
pub(crate) mod command;
pub(crate) mod github;
#[cfg(windows)]
mod handoff;
mod installation;
pub(crate) mod recovery;
pub(crate) mod service;
mod smoke;
mod trust;

pub(crate) use check::execute;
