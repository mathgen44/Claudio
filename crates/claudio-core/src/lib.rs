//! Socle commun de Claudio : chemins, configuration, journaux, secrets et base de données.

pub mod audit;
pub mod config;
pub mod db;
pub mod logging;
pub mod paths;
pub mod secrets;
pub mod usage;

pub use config::Config;
pub use db::{Database, Message, MessageMeta, Role, Session};
pub use paths::AppPaths;
