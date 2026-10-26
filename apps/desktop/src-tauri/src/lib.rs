//! Application de bureau : expose les services de Claudio à l'interface via des
//! commandes Tauri. Toute la logique vit dans `claudio-app`.

use std::sync::Arc;

use claudio_app::claudio_core::audit::AuditEntry;
use claudio_app::claudio_core::config::{
    ActiveModel, Budget, CloudModel, McpServerConfig, McpTransport, Permissions, ProviderKind,
    RoutingMode,
};
use claudio_app::claudio_core::{AppPaths, Config, Message, Session};
use claudio_app::claudio_llm::providers::RemoteModel;
use claudio_app::claudio_llm::runtime::{InstallEvent, InstalledRuntime};
use claudio_app::claudio_models::hf::ModelSummary;
use claudio_app::claudio_models::{InstalledModel, Progress};
use claudio_app::claudio_perms::Answer;
use claudio_app::claudio_tools::mcp::McpTool;
use claudio_app::{
    AccessOverview, BudgetStatus, Claudio, ProviderView, Recommendation, Reply, RepoView, Status,
    StreamEvent, UsageReport,
};
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
        "workspace" => app.paths.workspace_dir(),
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
    on_event: Channel<StreamEvent>,
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

#[tauri::command]
fn set_session_route(app: App<'_>, session_id: String, mode: Option<String>) -> CmdResult<()> {
    app.set_session_route(&session_id, mode.as_deref())
        .map_err(err)
}

// ---- Hugging Face -----------------------------------------------------------

#[tauri::command]
async fn hf_search(app: App<'_>, query: String) -> CmdResult<Vec<ModelSummary>> {
    app.hf_search(&query).await.map_err(err)
}

#[tauri::command]
async fn hf_repo(app: App<'_>, repo: String) -> CmdResult<RepoView> {
    app.hf_repo(&repo).await.map_err(err)
}

#[tauri::command]
async fn install_hf(
    app: App<'_>,
    repo: String,
    quant: String,
    on_progress: Channel<Progress>,
) -> CmdResult<InstalledModel> {
    app.install_hf(&repo, &quant, |p| {
        let _ = on_progress.send(p.clone());
    })
    .await
    .map_err(err)
}

// ---- Fournisseurs, routage, budgets ------------------------------------------

#[tauri::command]
fn list_providers(app: App<'_>) -> Vec<ProviderView> {
    app.providers()
}

#[tauri::command]
fn add_provider(
    app: App<'_>,
    kind: ProviderKind,
    name: String,
    base_url: Option<String>,
    api_key: Option<String>,
) -> CmdResult<ProviderView> {
    app.add_provider(kind, &name, base_url.as_deref(), api_key.as_deref())
        .map_err(err)
}

/// `base_url` vide = adresse par défaut du fournisseur.
#[tauri::command]
fn update_provider(
    app: App<'_>,
    id: String,
    name: Option<String>,
    base_url: Option<String>,
    enabled: Option<bool>,
) -> CmdResult<()> {
    let base_url = base_url
        .as_deref()
        .map(|u| Some(u).filter(|u| !u.trim().is_empty()));
    app.update_provider(&id, name.as_deref(), base_url, enabled)
        .map_err(err)
}

#[tauri::command]
fn set_provider_key(app: App<'_>, id: String, key: Option<String>) -> CmdResult<()> {
    app.set_provider_key(&id, key.as_deref()).map_err(err)
}

#[tauri::command]
fn remove_provider(app: App<'_>, id: String) -> CmdResult<()> {
    app.remove_provider(&id).map_err(err)
}

#[tauri::command]
async fn provider_models(app: App<'_>, id: String) -> CmdResult<Vec<RemoteModel>> {
    app.provider_models(&id).await.map_err(err)
}

#[tauri::command]
fn set_model_price(
    app: App<'_>,
    provider_id: String,
    model: String,
    input: f64,
    output: f64,
) -> CmdResult<()> {
    app.set_model_price(&provider_id, &model, input, output)
        .map_err(err)
}

#[tauri::command]
fn set_cloud_model(
    app: App<'_>,
    model: Option<CloudModel>,
    price: Option<(f64, f64)>,
) -> CmdResult<()> {
    app.set_cloud_model(model, price).map_err(err)
}

#[tauri::command]
fn set_routing_mode(app: App<'_>, mode: RoutingMode) -> CmdResult<()> {
    app.set_routing_mode(mode).map_err(err)
}

#[tauri::command]
fn set_budget(app: App<'_>, budget: Budget) -> CmdResult<()> {
    app.set_budget(budget).map_err(err)
}

#[tauri::command]
fn budget_status(app: App<'_>) -> CmdResult<BudgetStatus> {
    app.budget_status().map_err(err)
}

#[tauri::command]
fn usage_report(app: App<'_>) -> CmdResult<UsageReport> {
    app.usage_report().map_err(err)
}

// ---- Outils et accès -----------------------------------------------------------

#[tauri::command]
fn answer_approval(app: App<'_>, id: String, answer: Answer) -> CmdResult<()> {
    app.answer_approval(&id, answer).map_err(err)
}

#[tauri::command]
fn access_overview(app: App<'_>) -> CmdResult<AccessOverview> {
    app.access_overview().map_err(err)
}

#[tauri::command]
fn grant_folder(app: App<'_>, path: String, write: bool) -> CmdResult<()> {
    app.grant_folder(std::path::Path::new(&path), write)
        .map_err(err)
}

#[tauri::command]
fn revoke_grant(app: App<'_>, id: i64) -> CmdResult<()> {
    app.revoke_grant(id).map_err(err)
}

#[tauri::command]
fn set_permissions(app: App<'_>, permissions: Permissions) -> CmdResult<()> {
    app.set_permissions(permissions).map_err(err)
}

#[tauri::command]
fn set_tools_enabled(app: App<'_>, enabled: bool) -> CmdResult<()> {
    app.set_tools_enabled(enabled).map_err(err)
}

#[tauri::command]
fn audit_log(app: App<'_>, limit: u32, before: Option<i64>) -> CmdResult<Vec<AuditEntry>> {
    app.audit_log(limit, before).map_err(err)
}

#[tauri::command]
fn verify_audit(app: App<'_>) -> CmdResult<Option<i64>> {
    app.verify_audit().map_err(err)
}

#[tauri::command]
fn add_mcp_server(
    app: App<'_>,
    name: String,
    transport: McpTransport,
) -> CmdResult<McpServerConfig> {
    app.add_mcp_server(&name, transport).map_err(err)
}

#[tauri::command]
fn set_mcp_enabled(app: App<'_>, id: String, enabled: bool) -> CmdResult<()> {
    app.set_mcp_enabled(&id, enabled).map_err(err)
}

#[tauri::command]
async fn remove_mcp_server(app: App<'_>, id: String) -> CmdResult<()> {
    app.remove_mcp_server(&id).await.map_err(err)
}

#[tauri::command]
async fn test_mcp_server(app: App<'_>, id: String) -> CmdResult<Vec<McpTool>> {
    app.test_mcp_server(&id).await.map_err(err)
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
        .plugin(tauri_plugin_dialog::init())
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
            set_session_route,
            hf_search,
            hf_repo,
            install_hf,
            list_providers,
            add_provider,
            update_provider,
            set_provider_key,
            remove_provider,
            provider_models,
            set_model_price,
            set_cloud_model,
            set_routing_mode,
            set_budget,
            budget_status,
            usage_report,
            answer_approval,
            access_overview,
            grant_folder,
            revoke_grant,
            set_permissions,
            set_tools_enabled,
            audit_log,
            verify_audit,
            add_mcp_server,
            set_mcp_enabled,
            remove_mcp_server,
            test_mcp_server,
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
