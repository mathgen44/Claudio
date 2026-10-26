//! Application de bureau : expose les services de Claudio à l'interface via des
//! commandes Tauri. Toute la logique vit dans `claudio-app`.

use std::sync::Arc;

use claudio_app::claudio_core::config::ActiveModel;
use claudio_app::claudio_core::{AppPaths, Config, Message, Session};
use claudio_app::claudio_llm::ChatEvent;
use claudio_app::claudio_llm::runtime::{InstallEvent, InstalledRuntime};
use claudio_app::claudio_models::{InstalledModel, Progress};
use claudio_app::{Claudio, Recommendation, Reply, Status};
use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::{Manager, State};

type App<'a> = State<'a, Arc<Claudio>>;
type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    tracing::warn!("{e:#}");
    format!("{e:#}")
}

#[tauri::command]
async fn get_status(app: App<'_>) -> CmdResult<Status> {
    app.status().await.map_err(err)
}

#[tauri::command]
fn refresh_hardware(app: App<'_>) {
    app.refresh_hardware();
}

#[tauri::command]
fn get_recommendations(app: App<'_>) -> CmdResult<Vec<Recommendation>> {
    app.recommendations().map_err(err)
}

#[tauri::command]
async fn install_runtime(
    app: App<'_>,
    on_event: Channel<InstallEvent>,
) -> CmdResult<InstalledRuntime> {
    app.install_runtime(|ev| {
        let _ = on_event.send(ev);
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn install_model(
    app: App<'_>,
    model_id: String,
    quant: String,
    on_progress: Channel<Progress>,
) -> CmdResult<InstalledModel> {
    app.install_model(&model_id, &quant, |p| {
        let _ = on_progress.send(p.clone());
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn cancel_download(app: App<'_>, key: String) {
    app.cancel_download(&key);
}

#[tauri::command]
async fn remove_model(app: App<'_>, key: String) -> CmdResult<()> {
    app.remove_model(&key).await.map_err(err)
}

#[tauri::command]
fn set_active_model(app: App<'_>, model: ActiveModel) -> CmdResult<()> {
    app.set_active_model(model).map_err(err)
}

/// Charge le modèle actif en mémoire pour que la première réponse soit rapide.
#[tauri::command]
async fn warm_up(app: App<'_>) -> CmdResult<()> {
    app.active_endpoint().await.map(|_| ()).map_err(err)
}

#[tauri::command]
fn get_config(app: App<'_>) -> Config {
    app.config()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPatch {
    context_size: Option<Option<u32>>,
    use_ollama: Option<bool>,
    system_prompt: Option<String>,
    llama_backend: Option<Option<String>>,
    onboarding_done: Option<bool>,
    thinking: Option<Option<bool>>,
}

#[tauri::command]
fn update_settings(app: App<'_>, patch: SettingsPatch) -> CmdResult<Config> {
    app.update_config(|c| {
        if let Some(v) = patch.context_size {
            c.context_size = v;
        }
        if let Some(v) = patch.use_ollama {
            c.use_ollama = v;
        }
        if let Some(v) = patch.system_prompt {
            c.system_prompt = v;
        }
        if let Some(v) = patch.llama_backend {
            c.llama_backend = v;
        }
        if let Some(v) = patch.onboarding_done {
            c.onboarding_done = v;
        }
        if let Some(v) = patch.thinking {
            c.thinking = v;
        }
    })
    .map_err(err)
}

#[tauri::command]
fn set_hf_token(app: App<'_>, token: Option<String>) -> CmdResult<()> {
    app.set_hf_token(token.as_deref()).map_err(err)
}

#[tauri::command]
fn open_folder(app: App<'_>, which: String) -> CmdResult<()> {
    let path = match which.as_str() {
        "logs" => app.paths.logs_dir(),
        "models" => app.registry().root().to_path_buf(),
        "data" => app.paths.data_dir.clone(),
        _ => return Err("dossier inconnu".into()),
    };
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_sessions(app: App<'_>) -> CmdResult<Vec<Session>> {
    app.db.list_sessions().map_err(err)
}

#[tauri::command]
fn create_session(app: App<'_>) -> CmdResult<Session> {
    app.create_session().map_err(err)
}

#[tauri::command]
fn rename_session(app: App<'_>, id: String, title: String) -> CmdResult<()> {
    app.db.rename_session(&id, title.trim()).map_err(err)
}

#[tauri::command]
fn delete_session(app: App<'_>, id: String) -> CmdResult<()> {
    app.db.delete_session(&id).map_err(err)
}

#[tauri::command]
fn list_messages(app: App<'_>, session_id: String) -> CmdResult<Vec<Message>> {
    app.db.list_messages(&session_id).map_err(err)
}

#[tauri::command]
async fn send_message(
    app: App<'_>,
    session_id: String,
    text: String,
    on_event: Channel<ChatEvent>,
) -> CmdResult<Reply> {
    app.send_message(&session_id, &text, |ev| {
        let _ = on_event.send(ev);
    })
    .await
    .map_err(err)
}

#[tauri::command]
fn cancel_chat(app: App<'_>, session_id: String) {
    app.cancel_chat(&session_id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = AppPaths::resolve().expect("dossiers de Claudio");
    paths.ensure().expect("création des dossiers de Claudio");
    let guard = claudio_app::claudio_core::logging::init(&paths.logs_dir());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "démarrage de Claudio");
    let claudio = Claudio::open(paths).expect("initialisation de Claudio");

    tauri::Builder::default()
        // Une seule instance : relancer Claudio ramène la fenêtre existante.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(claudio)
        .manage(guard)
        .invoke_handler(tauri::generate_handler![
            get_status,
            refresh_hardware,
            get_recommendations,
            install_runtime,
            install_model,
            cancel_download,
            remove_model,
            set_active_model,
            warm_up,
            get_config,
            update_settings,
            set_hf_token,
            open_folder,
            list_sessions,
            create_session,
            rename_session,
            delete_session,
            list_messages,
            send_message,
            cancel_chat,
        ])
        .build(tauri::generate_context!())
        .expect("construction de l'application")
        .run(|handle, event| {
            if let tauri::RunEvent::Exit = event {
                let app = handle.state::<Arc<Claudio>>().inner().clone();
                tauri::async_runtime::block_on(app.shutdown());
            }
        });
}
