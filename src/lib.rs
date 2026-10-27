//! # Thulli Core Library
//!
//! A modular, reusable desktop productivity toolkit written in 100% pure Rust.
//! Provides domain entities, encrypted storage, smart task parsing,
//! focus timers, and native window management.

pub mod models;
pub mod parser;
pub mod repository;
pub mod security;
pub mod services;
pub mod window;

pub use models::{AppStateStore, BlockedApp, Task, TaskList, ThulliPreferences};
pub use parser::{parse_task_title, ParsedTaskTitle};
pub use repository::{EncryptedStorageRepository, StorageRepository};
pub use security::SecurityManager;
pub use services::{ListService, TaskService, TimerService};
pub use window::WindowManager;
