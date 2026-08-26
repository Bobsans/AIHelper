//! The platform-specific primitives AIHelper needs, behind one signature each.
//!
//! These were written inline, at the point of use, five and six times over. The
//! reparse-point check alone existed in five copies with three different
//! spellings of the same constant, and it is a *security* check: it is what
//! stops an installed file from being a link to somewhere else. A fix applied
//! to one copy left the other four wrong.
//!
//! Every function here returns [`std::io::Result`] and says nothing about what
//! the caller is doing. Callers have their own error types and their own
//! wording; what they were duplicating is the mechanism.

pub mod fs;
