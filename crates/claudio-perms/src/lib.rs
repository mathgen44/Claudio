//! Permissions des outils : moteur d'autorisation, chemins et bac à sable.

pub mod engine;
pub mod paths;
pub mod sandbox;

pub use engine::{Action, Answer, Approval, Check, Engine, Protected};
