//! UI wiring layer. One file per screen, each exposing
//! `pub fn bind(window, ctx)`, plus the shell and shared context.
//!
//! This module lives in the binary crate because it needs the Slint-generated
//! types; domain logic stays in `thulli_core`.

pub mod assistant;
pub mod calendar;
pub mod context;
pub mod convert;
pub mod focus;
pub mod insights;
pub mod lists;
pub mod matrix;
pub mod notes;
pub mod preferences;
pub mod reports;
pub mod shell;
pub mod shield;
pub mod tasks;
pub mod today;

pub use context::AppContext;
