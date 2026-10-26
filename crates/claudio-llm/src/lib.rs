//! Moteurs de modèles de langage : llama.cpp embarqué, Ollama et API compatibles OpenAI.

pub mod chat;
pub mod engine;
pub mod ollama;
mod process;
pub mod runtime;
pub mod server;

pub use chat::{ChatEvent, ChatMessage, Completion, Endpoint, Options};
pub use engine::LocalEngine;
pub use server::{LlamaServer, ServerConfig};
