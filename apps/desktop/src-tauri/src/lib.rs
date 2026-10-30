//! Application de bureau : expose les services de Claudio à l'interface via des
//! commandes Tauri. Toute la logique vit dans `claudio-app`.

use std::sync::Arc;

use claudio_app::claudio_agents::AgentDef;
use claudio_app::claudio_core::audit::AuditEntry;
use claudio_app::claudio_core::config::{
    ActiveModel, Budget, CloudModel, McpServerConfig, McpTransport, Permissions, ProviderKind,
    RoutingMode,
};
use claudio_app::claudio_core::config::{MemorySettings, VoiceSettings};
use claudio_app::claudio_core::tasks::{Schedule, Task, TaskStatus};
use claudio_app::claudio_core::{AppPaths, Config, Message, Session};
use claudio_app::claudio_llm::providers::RemoteModel;
use claudio_app::claudio_llm::runtime::{InstallEvent, InstalledRuntime};
use claudio_app::claudio_memory::index::Hit;
use claudio_app::claudio_memory::memories::Memory;
use claudio_app::claudio_memory::skills::{Proposal, Skill};
use claudio_app::claudio_models::hf::ModelSummary;
use claudio_app::claudio_models::{InstalledModel, Progress};
use claudio_app::claudio_perms::Answer;
use claudio_app::claudio_tools::mcp::McpTool;
use claudio_app::claudio_voice::tts::SystemVoice;
use claudio_app::{
    AccessOverview, AppEvent, BudgetStatus, Claudio, KnowledgeStatus, ProviderView, Recommendation,
    Reply, RepoView, ScheduleView, Status, StreamEvent, TaskDetail, TelegramStatus, Transcript,
    UsageReport, VoiceStatus,
};
use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;

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

#[tauri::command]
fn set_agent_model(app: App<'_>, agent_id: String, model: Option<ActiveModel>) -> CmdResult<()> {
    app.set_agent_model(&agent_id, model).map_err(err)
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
    check_updates: Option<bool>,
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
        if let Some(v) = patch.check_updates {
            c.check_updates = v;
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
        "agents" => app.paths.agents_dir(),
        "skills" => app.paths.skills_dir(),
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
async fn read_attachment(app: App<'_>, path: String) -> CmdResult<claudio_app::Attachment> {
    app.read_attachment(&path).await.map_err(err)
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
async fn add_mcp_server(
    app: App<'_>,
    name: String,
    transport: McpTransport,
    secrets: Vec<(String, String)>,
) -> CmdResult<McpServerConfig> {
    app.add_mcp_server(&name, transport, secrets)
        .await
        .map_err(err)
}

#[tauri::command]
async fn set_mcp_secrets(
    app: App<'_>,
    id: String,
    secrets: Vec<(String, String)>,
) -> CmdResult<McpServerConfig> {
    app.set_mcp_secrets(&id, secrets).await.map_err(err)
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

// ---- Agents, tâches, planifications ------------------------------------------

#[derive(serde::Serialize)]
struct AgentsView {
    agents: Vec<AgentDef>,
    /// Fichiers ignorés : (nom, raison).
    errors: Vec<(String, String)>,
}

#[tauri::command]
fn list_agents(app: App<'_>, reload: bool) -> AgentsView {
    let set = if reload {
        app.reload_agents()
    } else {
        app.agents()
    };
    AgentsView {
        agents: set.agents,
        errors: set.errors,
    }
}

fn agents_view(set: claudio_app::claudio_agents::AgentSet) -> AgentsView {
    AgentsView {
        agents: set.agents,
        errors: set.errors,
    }
}

#[tauri::command]
fn save_agent(app: App<'_>, agent: AgentDef, create: bool) -> CmdResult<AgentsView> {
    app.save_agent(&agent, create).map(agents_view).map_err(err)
}

#[tauri::command]
fn delete_agent(app: App<'_>, id: String) -> CmdResult<AgentsView> {
    app.delete_agent(&id).map(agents_view).map_err(err)
}

#[tauri::command]
async fn available_tools(app: App<'_>) -> CmdResult<Vec<(String, String)>> {
    Ok(app.available_tools().await)
}

#[tauri::command]
fn list_tasks(app: App<'_>) -> CmdResult<Vec<Task>> {
    app.list_tasks(100).map_err(err)
}

#[tauri::command]
fn task_detail(app: App<'_>, id: String) -> CmdResult<TaskDetail> {
    app.task_detail(&id).map_err(err)
}

#[tauri::command]
async fn start_task(app: App<'_>, objective: String, mode: String) -> CmdResult<Task> {
    app.inner().start_task(&objective, &mode, None).map_err(err)
}

#[tauri::command]
fn cancel_task(app: App<'_>, id: String) {
    app.cancel_task(&id);
}

#[tauri::command]
async fn resume_task(app: App<'_>, id: String) -> CmdResult<()> {
    app.inner().resume_task(&id).map_err(err)
}

#[tauri::command]
fn delete_task(app: App<'_>, id: String) -> CmdResult<()> {
    app.delete_task(&id).map_err(err)
}

#[tauri::command]
fn list_schedules(app: App<'_>) -> CmdResult<Vec<ScheduleView>> {
    app.schedules().map_err(err)
}

#[tauri::command]
fn add_schedule(
    app: App<'_>,
    title: String,
    objective: String,
    mode: String,
    cron: String,
) -> CmdResult<Schedule> {
    app.add_schedule(&title, &objective, &mode, &cron)
        .map_err(err)
}

#[tauri::command]
fn set_schedule_enabled(app: App<'_>, id: i64, enabled: bool) -> CmdResult<()> {
    app.set_schedule_enabled(id, enabled).map_err(err)
}

#[tauri::command]
fn delete_schedule(app: App<'_>, id: i64) -> CmdResult<()> {
    app.delete_schedule(id).map_err(err)
}

#[tauri::command]
async fn run_schedule_now(app: App<'_>, id: i64) -> CmdResult<String> {
    app.inner().run_schedule_now(id).map_err(err)
}

// ---- Mémoire, documents, compétences --------------------------------------------

#[tauri::command]
fn knowledge_status(app: App<'_>) -> CmdResult<KnowledgeStatus> {
    app.knowledge_status().map_err(err)
}

#[tauri::command]
async fn add_index_source(app: App<'_>, path: String) -> CmdResult<i64> {
    app.inner()
        .add_index_source(std::path::Path::new(&path))
        .map_err(err)
}

#[tauri::command]
async fn remove_index_source(app: App<'_>, id: i64) -> CmdResult<()> {
    app.inner().remove_index_source(id).map_err(err)
}

#[tauri::command]
async fn reindex_source(app: App<'_>, id: i64) -> CmdResult<()> {
    app.inner().reindex(id);
    Ok(())
}

#[tauri::command]
async fn reindex_all(app: App<'_>) -> CmdResult<()> {
    app.inner().reindex_all().map_err(err)
}

#[tauri::command]
async fn search_documents(app: App<'_>, query: String) -> CmdResult<Vec<Hit>> {
    app.search_documents(&query, 8).await.map_err(err)
}

#[tauri::command]
fn list_memories(app: App<'_>, filter: Option<String>) -> CmdResult<Vec<Memory>> {
    app.memories(filter.as_deref()).map_err(err)
}

#[tauri::command]
async fn add_memory(app: App<'_>, content: String, category: String) -> CmdResult<Memory> {
    app.add_memory(&content, &category).await.map_err(err)
}

#[tauri::command]
async fn update_memory(
    app: App<'_>,
    id: i64,
    content: String,
    category: String,
    pinned: bool,
) -> CmdResult<Memory> {
    app.update_memory(id, &content, &category, pinned)
        .await
        .map_err(err)
}

#[tauri::command]
fn delete_memory(app: App<'_>, id: i64) -> CmdResult<()> {
    app.delete_memory(id).map_err(err)
}

#[tauri::command]
fn delete_all_memories(app: App<'_>) -> CmdResult<usize> {
    app.delete_all_memories().map_err(err)
}

#[tauri::command]
fn set_memory_settings(app: App<'_>, settings: MemorySettings) -> CmdResult<()> {
    app.update_config(|c| c.memory = settings)
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
fn list_skills(app: App<'_>) -> Vec<Skill> {
    app.skills()
}

#[tauri::command]
fn save_skill(app: App<'_>, name: String, description: String, body: String) -> CmdResult<String> {
    app.save_skill(&name, &description, &body).map_err(err)
}

#[tauri::command]
fn delete_skill(app: App<'_>, name: String) -> CmdResult<()> {
    app.delete_skill(&name).map_err(err)
}

#[tauri::command]
fn skill_proposals(app: App<'_>) -> CmdResult<Vec<Proposal>> {
    app.skill_proposals().map_err(err)
}

#[tauri::command]
fn accept_skill(
    app: App<'_>,
    id: i64,
    name: String,
    description: String,
    body: String,
) -> CmdResult<String> {
    app.accept_skill(id, &name, &description, &body)
        .map_err(err)
}

#[tauri::command]
fn reject_skill(app: App<'_>, id: i64) -> CmdResult<()> {
    app.reject_skill(id).map_err(err)
}

#[tauri::command]
fn voice_status(app: App<'_>) -> VoiceStatus {
    app.voice_status()
}

#[tauri::command]
async fn install_voice(app: App<'_>, id: String, on_progress: Channel<Progress>) -> CmdResult<()> {
    app.install_voice(&id, |p| {
        let _ = on_progress.send(p.clone());
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn remove_voice(app: App<'_>, id: String) -> CmdResult<()> {
    app.remove_voice(&id).await.map_err(err)
}

#[tauri::command]
async fn system_voices(app: App<'_>) -> CmdResult<Vec<SystemVoice>> {
    app.system_voices().await.map_err(err)
}

#[tauri::command]
async fn set_voice_settings(app: App<'_>, settings: VoiceSettings) -> CmdResult<()> {
    let wake = settings.wake_word;
    app.update_config(|c| c.voice = settings).map_err(err)?;
    if !wake {
        // Libère la mémoire des modèles vocaux ; ils redémarrent à la prochaine dictée.
        app.stop_voice().await;
    }
    Ok(())
}

/// L'audio (WAV) arrive brut dans le corps de la requête ; l'en-tête `wake`
/// indique une écoute du mot d'éveil.
#[tauri::command]
async fn transcribe(app: App<'_>, request: tauri::ipc::Request<'_>) -> CmdResult<Transcript> {
    let tauri::ipc::InvokeBody::Raw(wav) = request.body() else {
        return Err("audio attendu".into());
    };
    let wake = request.headers().get("wake").is_some_and(|v| v == "1");
    app.transcribe(wav.clone(), wake).await.map_err(err)
}

/// Renvoie l'audio WAV brut (ArrayBuffer côté interface).
#[tauri::command]
async fn synthesize(app: App<'_>, text: String) -> CmdResult<tauri::ipc::Response> {
    app.synthesize(&text)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(err)
}

#[tauri::command]
fn telegram_status(app: App<'_>) -> TelegramStatus {
    app.telegram_status()
}

#[tauri::command]
async fn set_telegram_token(app: App<'_>, token: Option<String>) -> CmdResult<()> {
    app.set_telegram_token(token.as_deref()).await.map_err(err)
}

#[tauri::command]
async fn set_telegram_enabled(app: App<'_>, enabled: bool) -> CmdResult<()> {
    app.set_telegram_enabled(enabled).map_err(err)
}

#[tauri::command]
fn telegram_pairing_code(app: App<'_>) -> String {
    app.telegram_pairing_code()
}

#[tauri::command]
fn remove_telegram_chat(app: App<'_>, id: i64) -> CmdResult<()> {
    app.remove_telegram_chat(id).map_err(err)
}

/// Argument passé par le lancement automatique avec Windows.
const MINIMIZED_ARG: &str = "--minimized";

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Événements des tâches vers l'interface, et notification Windows à la fin d'une tâche.
fn install_event_sink(handle: &AppHandle, claudio: &Arc<Claudio>) {
    let h = handle.clone();
    claudio.set_event_sink(Arc::new(move |ev: AppEvent| {
        match &ev {
            AppEvent::TaskFinished { title, status, .. } => {
                let body = match status {
                    TaskStatus::Done => "Terminée.",
                    TaskStatus::Partial => "Terminée, avec des étapes en échec.",
                    TaskStatus::Cancelled => "Annulée.",
                    _ => "En échec.",
                };
                let _ = h.notification().builder().title(title).body(body).show();
            }
            AppEvent::Approval { task_title, .. } => {
                let _ = h
                    .notification()
                    .builder()
                    .title("Claudio attend votre accord")
                    .body(task_title)
                    .show();
            }
            _ => {}
        }
        let _ = h.emit("claudio://event", ev);
    }));
}

/// Icône de la zone de notification : Claudio continue ses tâches fenêtre fermée.
fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Ouvrir Claudio", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quitter Claudio", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("claudio")
        .tooltip("Claudio")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
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
            show_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Lancement à l'ouverture de session : discret, dans la zone de notification.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args([MINIMIZED_ARG])
                .build(),
        )
        .setup({
            let claudio = claudio.clone();
            move |app| {
                install_event_sink(app.handle(), &claudio);
                // La fenêtre est créée cachée : montrée sauf démarrage avec Windows.
                if !std::env::args().any(|a| a == MINIMIZED_ARG) {
                    show_window(app.handle());
                }
                install_tray(app)?;
                tauri::async_runtime::spawn(claudio.clone().scheduler_loop());
                // Index : remise à jour au démarrage, puis surveillance des dossiers.
                let c = claudio.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = c.reindex_all() {
                        tracing::warn!("réindexation : {e:#}");
                    }
                    c.refresh_watchers();
                });
                let c = claudio.clone();
                tauri::async_runtime::spawn(async move { c.start_telegram() });
                Ok(())
            }
        })
        .on_window_event(|window, event| {
            // Fermer la fenêtre la cache : les tâches et planifications continuent.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
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
            set_agent_model,
            save_agent,
            delete_agent,
            available_tools,
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
            read_attachment,
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
            set_mcp_secrets,
            set_mcp_enabled,
            remove_mcp_server,
            test_mcp_server,
            list_agents,
            list_tasks,
            task_detail,
            start_task,
            cancel_task,
            resume_task,
            delete_task,
            list_schedules,
            add_schedule,
            set_schedule_enabled,
            delete_schedule,
            run_schedule_now,
            knowledge_status,
            add_index_source,
            remove_index_source,
            reindex_source,
            reindex_all,
            search_documents,
            list_memories,
            add_memory,
            update_memory,
            delete_memory,
            delete_all_memories,
            set_memory_settings,
            list_skills,
            save_skill,
            delete_skill,
            skill_proposals,
            accept_skill,
            reject_skill,
            voice_status,
            install_voice,
            remove_voice,
            system_voices,
            set_voice_settings,
            transcribe,
            synthesize,
            telegram_status,
            set_telegram_token,
            set_telegram_enabled,
            telegram_pairing_code,
            remove_telegram_chat,
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
