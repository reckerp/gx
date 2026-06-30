//! Git-side support for `gx review`: resolving the diff range a review targets
//! (and, in later units, building the structured diff model and persisting
//! review state).

pub mod blob;
pub mod diff;
pub mod range;
pub mod state;
