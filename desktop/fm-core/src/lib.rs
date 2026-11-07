//! Figura Munda: shows you what a file reveals about you, then removes it.
//!
//! Inspection is the product and cleaning is the side effect, so the engine is
//! built in that order: [`format`] handlers map a file's fields onto the ten
//! classes of [`model`], a [`profile::Profile`] says what to do with each class,
//! and [`report`] renders the four visible artefacts (report, diff, verify,
//! recipient view).
//!
//! Nothing in this crate opens a socket.

pub mod anonset;
pub mod error;
pub mod format;
pub mod fsmeta;
pub mod model;
pub mod pipeline;
pub mod profile;
pub mod report;
pub mod scan;
pub mod timeutil;

pub use error::{Error, Result};
pub use model::{Action, Class, Finding, FormatId, Impact, Inspection};
pub use pipeline::{clean_bytes, clean_file, inspect_bytes, inspect_file, verify_bytes, Cleaned};
pub use profile::Profile;
