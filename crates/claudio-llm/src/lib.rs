//! Moteurs de modèles de langage : llama.cpp embarqué, Ollama et API compatibles OpenAI.

pub mod anthropic;
pub mod chat;
pub mod embed;
pub mod engine;
pub mod ollama;
mod process;
pub mod providers;
pub mod runtime;
pub mod server;

pub use chat::{Api, ChatEvent, ChatMessage, Completion, Endpoint, Options};
pub use engine::LocalEngine;
pub use server::{LlamaServer, ServerConfig};
