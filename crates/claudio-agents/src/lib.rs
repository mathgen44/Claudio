//! Agents : définitions YAML et plans d'exécution en graphe.

pub mod defs;
pub mod plan;

pub use defs::{AgentDef, AgentSet, ModelPref};
pub use plan::{Plan, PlanStep};
