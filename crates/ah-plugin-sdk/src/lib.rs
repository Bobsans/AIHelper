//! Shared implementation support for AIHelper plugins.
//!
//! `ah-plugin-api` defines the *contract* a plugin must satisfy. This crate
//! carries the *implementation* every plugin would otherwise write again, so a
//! bug in it is fixed in one place rather than once per plugin. The C ABI is
//! unchanged: this is a Rust-side convenience, and a plugin built without it
//! still loads.

pub mod credentials;
pub mod git;
pub mod http;
pub mod logs;
pub mod poll;
pub mod render;
pub mod text;
