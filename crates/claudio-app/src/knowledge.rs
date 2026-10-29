//! Connaissances : embeddings locaux, index des dossiers (avec surveillance des
//! modifications), mémoire à long terme, compétences, et les outils qui les
//! exposent aux modèles.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use claudio_core::audit::AuditRecord;
use claudio_core::config::{ActiveModel, RoutingMode};
use claudio_llm::ServerConfig;
use claudio_llm::chat::{ChatMessage, ToolDef};
use claudio_llm::runtime;
use claudio_memory::index::{self, Hit, SourceView};
use claudio_memory::memories::{self, Memory};
use claudio_memory::skills::{self, Proposal, Skill};
use claudio_memory::{BoxFuture, Embedder};
use claudio_models::{CatalogModel, InstalledModel};
use claudio_tools::{ToolOutput, untrusted};
use serde::Serialize;
use serde_json::{Value, json};

use crate::Claudio;
use crate::conversation::{LoopCtx, StreamEvent, clip};
use crate::tasks::AppEvent;

/// Outils fournis par l'application elle-même (mémoire, documents, compétences).
pub(crate) const APP_TOOLS: &[&str] = &["search_documents", "remember", "read_skill"];

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeStatus {
    /// Modèle d'embeddings installé (sinon la recherche se fait au mot près).
    pub embedding_model: Option<String>,
    /// Modèle d'embeddings recommandé à installer.
    pub recommended: Option<(String, String)>,
    pub sources: Vec<SourceView>,
    pub pending_embeddings: i64,
}

/// Embeddings calculés par le modèle local installé.
pub(crate) struct LocalEmbedder<'a> {
    app: &'a Claudio,
    model: CatalogModel,
    installed: InstalledModel,
}

impl Embedder for LocalEmbedder<'_> {
    fn model(&self) -> String {
        self.installed.key.clone()
    }

    fn dims(&self) -> usize {
        self.model.dims.unwrap_or(1024) as usize
    }

    fn embed<'b>(
        &'b self,
        texts: &'b [String],
        query: bool,
    ) -> BoxFuture<'b, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            let rt = runtime::installed(&self.app.paths.runtimes_dir())
                .context("le moteur llama.cpp n'est pas installé")?;
            let profile = self.app.hardware().profile();
            let cfg = ServerConfig {
                exe: rt.server_exe(),
                model_path: self.app.registry().path_of(&self.installed),
                alias: self.installed.key.clone(),
                context: self.model.context_max.min(8192),
                // Sur le processeur : la carte graphique reste entière au modèle de
                // conversation (sinon llama.cpp en déplacerait une partie en RAM).
                use_gpu: false,
                threads: (profile.threads / 2).max(2),
                parallel: 2,
                embedding_pooling: Some(
                    self.model.pooling.clone().unwrap_or_else(|| "mean".into()),
                ),
                memory: 0,
            };
            let endpoint = self
                .app
                .embedder
                .ensure(&self.app.http, cfg, u64::MAX)
                .await?;
            let prefix = if query {
                self.model.query_prefix.clone().unwrap_or_default()
            } else {
                String::new()
            };
            let mut out = Vec::with_capacity(texts.len());
            for batch in texts.chunks(16) {
                let inputs: Vec<String> = batch
                    .iter()
                    // Un passage reste sous la taille de contexte du modèle.
                    .map(|t| format!("{prefix}{}", t.chars().take(6000).collect::<String>()))
                    .collect();
                out.extend(claudio_llm::embed::embed(&self.app.http, &endpoint, &inputs).await?);
            }
            Ok(out)
        })
    }
}

impl Claudio {
    /// Embedder local, si un modèle d'embeddings et le moteur sont installés.
    pub(crate) fn local_embedder(&self) -> Option<LocalEmbedder<'_>> {
        runtime::installed(&self.paths.runtimes_dir())?;
        let installed = self.registry().list().ok()?.into_iter().find(|m| {
            self.catalog
                .get(&m.model_id)
                .is_some_and(CatalogModel::is_embedding)
        })?;
        let model = self.catalog.get(&installed.model_id)?.clone();
        Some(LocalEmbedder {
            app: self,
            model,
            installed,
        })
    }

    pub fn knowledge_status(&self) -> Result<KnowledgeStatus> {
        let recommended = self
            .catalog
            .models
            .iter()
            .find(|m| m.is_embedding())
            .and_then(|m| m.quants.first().map(|q| (m.id.clone(), q.name.clone())));
        Ok(KnowledgeStatus {
            embedding_model: self.local_embedder().map(|e| e.installed.name.clone()),
            recommended,
            sources: index::sources(&self.db)?,
            pending_embeddings: index::pending_embeddings(&self.db)?,
        })
    }

    // ---- Index des dossiers -----------------------------------------------------

    /// Ajoute un dossier à l'index (il devient lisible par l'assistant) et l'indexe.
    pub fn add_index_source(self: &Arc<Self>, path: &Path) -> Result<i64> {
        self.perms.grant_folder(path, false)?;
        let canon = claudio_perms::paths::normalize(path)?;
        let id = index::add_source(&self.db, &canon)?;
        self.refresh_watchers();
        self.reindex(id);
        Ok(id)
    }

    pub fn remove_index_source(self: &Arc<Self>, id: i64) -> Result<()> {
        index::remove_source(&self.db, id)?;
        self.refresh_watchers();
        Ok(())
    }

    /// Relance l'indexation d'un dossier en arrière-plan (sans effet si elle tourne déjà).
    pub fn reindex(self: &Arc<Self>, source_id: i64) {
        if !self.indexing.lock().unwrap().insert(source_id) {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let res = app.index_job(source_id).await;
            app.indexing.lock().unwrap().remove(&source_id);
            if let Err(e) = &res {
                tracing::warn!(source_id, "indexation : {e:#}");
                let _ = index::set_status(&app.db, source_id, "error", Some(&format!("{e:#}")));
            }
            app.emit_event(AppEvent::IndexUpdated { source_id });
        });
    }

    async fn index_job(self: &Arc<Self>, source_id: i64) -> Result<()> {
        index::set_status(&self.db, source_id, "scanning", None)?;
        self.emit_event(AppEvent::IndexUpdated { source_id });
        let app = self.clone();
        let stats = tokio::task::spawn_blocking(move || {
            let stop = AtomicBool::new(false);
            let mut last = std::time::Instant::now();
            index::scan(&app.db, source_id, &stop, |done, total, path| {
                if last.elapsed() > Duration::from_millis(300) {
                    last = std::time::Instant::now();
                    app.emit_event(AppEvent::IndexProgress {
                        source_id,
                        done,
                        total,
                        path: path.to_owned(),
                    });
                }
            })
        })
        .await??;
        tracing::info!(source_id, ?stats, "dossier indexé");
        if let Some(e) = self.local_embedder() {
            index::set_status(&self.db, source_id, "embedding", None)?;
            self.emit_event(AppEvent::IndexUpdated { source_id });
            let stop = AtomicBool::new(false);
            index::embed_pending(&self.db, &e, &stop).await?;
            memories::embed_pending(&self.db, &e, &stop).await?;
        }
        index::set_status(&self.db, source_id, "idle", None)?;
        Ok(())
    }

    /// Réindexe tout (au démarrage, après l'installation du modèle d'embeddings…).
    pub fn reindex_all(self: &Arc<Self>) -> Result<()> {
        index::reset_statuses(&self.db)?;
        for s in index::sources(&self.db)?.into_iter().filter(|s| s.enabled) {
            self.reindex(s.id);
        }
        Ok(())
    }

    /// Surveille les dossiers indexés : une modification déclenche une réindexation.
    pub fn refresh_watchers(self: &Arc<Self>) {
        use notify_debouncer_mini::notify::RecursiveMode;
        let Ok(sources) = index::sources(&self.db) else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let app = Arc::downgrade(self);
        let roots: Vec<(i64, std::path::PathBuf)> = sources
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id, s.path.clone().into()))
            .collect();
        let watched = roots.clone();
        let debouncer = notify_debouncer_mini::new_debouncer(
            Duration::from_secs(5),
            move |res: notify_debouncer_mini::DebounceEventResult| {
                let Ok(events) = res else { return };
                let Some(app) = app.upgrade() else { return };
                let touched: HashSet<i64> = events
                    .iter()
                    .filter_map(|e| {
                        watched
                            .iter()
                            .find(|(_, root)| e.path.starts_with(root))
                            .map(|(id, _)| *id)
                    })
                    .collect();
                let _guard = handle.enter();
                for id in touched {
                    app.reindex(id);
                }
            },
        );
        let mut debouncer = match debouncer {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("surveillance des dossiers impossible : {e}");
                return;
            }
        };
        for (_, root) in &roots {
            if let Err(e) = debouncer.watcher().watch(root, RecursiveMode::Recursive) {
                tracing::warn!("surveillance de {} impossible : {e}", root.display());
            }
        }
        *self.watcher.lock().unwrap() = Some(debouncer);
    }

    pub async fn search_documents(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let e = self.local_embedder();
        index::search(
            &self.db,
            e.as_ref().map(|e| e as &dyn Embedder),
            query,
            k.clamp(1, 20),
        )
        .await
    }

    // ---- Mémoire -------------------------------------------------------------------

    pub fn memories(&self, filter: Option<&str>) -> Result<Vec<Memory>> {
        memories::list(&self.db, filter)
    }

    pub async fn add_memory(&self, content: &str, category: &str) -> Result<Memory> {
        let e = self.local_embedder();
        memories::add(
            &self.db,
            e.as_ref().map(|e| e as &dyn Embedder),
            content,
            category,
            "manuel",
            None,
        )
        .await
    }

    pub async fn update_memory(
        &self,
        id: i64,
        content: &str,
        category: &str,
        pinned: bool,
    ) -> Result<Memory> {
        let e = self.local_embedder();
        memories::update(
            &self.db,
            e.as_ref().map(|e| e as &dyn Embedder),
            id,
            content,
            category,
            pinned,
        )
        .await
    }

    pub fn delete_memory(&self, id: i64) -> Result<()> {
        memories::delete(&self.db, id)
    }

    pub fn delete_all_memories(&self) -> Result<usize> {
        memories::delete_all(&self.db)
    }

    /// Contexte ajouté aux consignes : souvenirs utiles, compétences, documents.
    pub(crate) async fn knowledge_context(&self, query: &str, tools: bool) -> String {
        let cfg = self.config();
        let mut out = String::new();
        if cfg.memory.recall {
            let e = self.local_embedder();
            match memories::recall(&self.db, e.as_ref().map(|e| e as &dyn Embedder), query, 6).await
            {
                Ok(m) if !m.is_empty() => {
                    out.push_str("\n\nCe que tu sais de l'utilisateur (souvenirs qu'il peut consulter et corriger) :");
                    for m in m {
                        out.push_str(&format!("\n- {}", m.content));
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("rappel des souvenirs : {e:#}"),
            }
        }
        if tools {
            let skills = skills::list(&self.paths.skills_dir());
            if !skills.is_empty() {
                out.push_str(
                    "\n\nCompétences disponibles (procédures validées par l'utilisateur ; lis-en une avec \
                     read_skill avant de l'appliquer) :",
                );
                for s in skills.iter().take(40) {
                    out.push_str(&format!("\n- {} : {}", s.name, s.description));
                }
            }
            if index::sources(&self.db).is_ok_and(|s| !s.is_empty()) {
                out.push_str(
                    "\n\nDes dossiers de l'utilisateur sont indexés : cherche-y avec search_documents avant \
                     de dire que tu ne sais pas, et cite le fichier source.",
                );
            }
            out.push_str(
                "\nSi l'utilisateur te demande de retenir quelque chose, utilise l'outil remember.",
            );
        }
        out
    }

    /// Définitions des outils de l'application.
    pub(crate) fn app_tool_defs(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "search_documents".into(),
                description:
                    "Cherche dans les documents indexés de l'utilisateur (texte, PDF, Office) ; \
                    renvoie les passages pertinents avec leur fichier."
                        .into(),
                parameters: json!({"type": "object", "properties": {
                    "query": {"type": "string", "description": "Question ou mots-clés"},
                    "k": {"type": "integer", "description": "Nombre de passages (6 par défaut)"}
                }, "required": ["query"]}),
            },
            ToolDef {
                name: "remember".into(),
                description:
                    "Retient durablement un fait ou une préférence de l'utilisateur (visible et \
                    modifiable par lui dans la page Mémoire)."
                        .into(),
                parameters: json!({"type": "object", "properties": {
                    "content": {"type": "string", "description": "Le fait, en une phrase autonome"},
                    "category": {"type": "string", "enum": memories::CATEGORIES}
                }, "required": ["content"]}),
            },
            ToolDef {
                name: "read_skill".into(),
                description: "Lit une compétence (procédure validée) par son nom.".into(),
                parameters: json!({"type": "object", "properties": {
                    "name": {"type": "string"}
                }, "required": ["name"]}),
            },
        ]
    }

    /// Exécute un outil de l'application et l'inscrit au journal.
    pub(crate) async fn call_app_tool(&self, scope: &str, name: &str, args: &str) -> ToolOutput {
        let v: Value = serde_json::from_str(if args.trim().is_empty() { "{}" } else { args })
            .unwrap_or(Value::Null);
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let res: Result<String> = async {
            match name {
                "search_documents" => {
                    let k = v.get("k").and_then(Value::as_u64).unwrap_or(6) as usize;
                    let hits = self.search_documents(&s("query"), k).await?;
                    if hits.is_empty() {
                        return Ok("Aucun passage trouvé dans les documents indexés.".into());
                    }
                    let body = hits
                        .iter()
                        .map(|h| {
                            format!(
                                "### {}{}\n{}",
                                h.path,
                                h.location
                                    .as_deref()
                                    .map(|l| format!(" ({l})"))
                                    .unwrap_or_default(),
                                clip(&h.text, 1500)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    Ok(untrusted("documents indexés", &body))
                }
                "remember" => {
                    let e = self.local_embedder();
                    let e = e.as_ref().map(|e| e as &dyn Embedder);
                    if let Some(e) = e
                        && let Some(dup) =
                            memories::find_duplicate(&self.db, e, &s("content")).await?
                    {
                        return Ok(format!("Déjà retenu : {}", dup.content));
                    }
                    let session =
                        (!scope.starts_with("task:") && !scope.is_empty()).then_some(scope);
                    let m = memories::add(
                        &self.db,
                        e,
                        &s("content"),
                        &s("category"),
                        "assistant",
                        session,
                    )
                    .await?;
                    self.emit_event(AppEvent::MemoryLearned {
                        content: m.content.clone(),
                    });
                    Ok(format!("Retenu : {}", m.content))
                }
                "read_skill" => Ok(skills::get(&self.paths.skills_dir(), &s("name"))?.body),
                _ => bail!("outil inconnu : {name}"),
            }
        }
        .await;
        let out = match res {
            Ok(t) => ToolOutput::ok(t),
            Err(e) => ToolOutput::error(format!("{e:#}")),
        };
        let _ = self.db.audit(&AuditRecord {
            session_id: Some(scope),
            tool: name,
            detail: &clip(args, 500),
            decision: "auto (outil interne)",
            outcome: if out.is_error { &out.content } else { "ok" },
        });
        out
    }

    /// Après une réponse : retient les faits durables (modèle local uniquement).
    pub(crate) fn spawn_memory_extraction(
        self: &Arc<Self>,
        session_id: String,
        user: String,
        reply: String,
    ) {
        let cfg = self.config();
        if !cfg.memory.auto_extract || !matches!(cfg.active_model, Some(ActiveModel::Local { .. }))
        {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            if let Err(e) = app.extract_memories(&session_id, &user, &reply).await {
                tracing::debug!("extraction de souvenirs : {e:#}");
            }
        });
    }

    async fn extract_memories(&self, session_id: &str, user: &str, reply: &str) -> Result<()> {
        // Les messages très courts ne contiennent presque jamais de fait durable.
        if user.chars().count() < 15 {
            return Ok(());
        }
        let schema = json!({
            "type": "object", "additionalProperties": false, "required": ["memories"],
            "properties": {"memories": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["content", "category"],
                "properties": {
                    "content": {"type": "string"},
                    "category": {"type": "string", "enum": memories::CATEGORIES}
                }
            }}}
        });
        // Ce qui est déjà connu, pour ne pas le répéter sous une autre forme.
        let e = self.local_embedder();
        let known: Vec<String> =
            memories::recall(&self.db, e.as_ref().map(|e| e as &dyn Embedder), user, 10)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|m| format!("- {}", m.content))
                .collect();
        let known = if known.is_empty() {
            "(aucun)".to_owned()
        } else {
            known.join("\n")
        };
        let prompt = format!(
            "Déjà retenu (ne le répète pas, même reformulé) :\n{known}\n\n\
             Message de l'utilisateur :\n{}\n\nRéponse de l'assistant :\n{}\n\n\
             Extrais au plus 3 faits DURABLES et NOUVEAUX sur l'utilisateur, qu'il a lui-même exprimés : préférences, \
             informations personnelles qu'il partage volontairement, projets en cours, habitudes. Pas de \
             demande ponctuelle, pas de connaissance générale, rien de ce que l'assistant a dit. Chaque fait \
             est une phrase autonome à la troisième personne (« L'utilisateur… »). Si rien ne mérite d'être \
             retenu, renvoie une liste vide. Réponds en JSON : {{\"memories\": [...]}}.",
            clip(user, 3000),
            clip(reply, 2000)
        );
        let messages = vec![
            ChatMessage::text(
                "system",
                "Tu extrais des souvenirs utiles et sûrs, sans rien inventer.",
            ),
            ChatMessage::text("user", prompt),
        ];
        let decision = self.route(RoutingMode::Local, &messages, None)?;
        let noop = |_: StreamEvent| {};
        let approver = crate::tasks::DenyAll;
        let cancel = claudio_models::CancelToken::default();
        let lc = LoopCtx {
            scope: "",
            approver: &approver,
            emit: &noop,
            cancel: &cancel,
            max_steps: 1,
            thinking: Some(false),
            json_schema: Some(schema),
            local_model: None,
        };
        let run = self.answer(&decision, &messages, &[], &lc).await?;
        self.record_run(&run, Some(session_id))?;
        let text = &run.content;
        tracing::debug!(sortie = %text, "extraction de souvenirs");
        let v: Value = serde_json::from_str(
            text.get(text.find('{').unwrap_or(0)..=text.rfind('}').unwrap_or(0))
                .unwrap_or("{}"),
        )
        .unwrap_or(Value::Null);
        let e = e.as_ref().map(|e| e as &dyn Embedder);
        for m in v["memories"].as_array().into_iter().flatten().take(3) {
            let content = m["content"].as_str().unwrap_or("").trim();
            if content.chars().count() < 8 {
                continue;
            }
            if let Some(e) = e
                && memories::find_duplicate(&self.db, e, content)
                    .await?
                    .is_some()
            {
                continue;
            }
            let added = memories::add(
                &self.db,
                e,
                content,
                m["category"].as_str().unwrap_or("autre"),
                "auto",
                Some(session_id),
            )
            .await?;
            tracing::info!(content = %added.content, "souvenir retenu");
            self.emit_event(AppEvent::MemoryLearned {
                content: added.content,
            });
        }
        Ok(())
    }

    // ---- Compétences ---------------------------------------------------------------

    pub fn skills(&self) -> Vec<Skill> {
        skills::list(&self.paths.skills_dir())
    }

    pub fn save_skill(&self, name: &str, description: &str, body: &str) -> Result<String> {
        skills::save(&self.paths.skills_dir(), name, description, body)
    }

    pub fn delete_skill(&self, name: &str) -> Result<()> {
        skills::delete(&self.paths.skills_dir(), name)
    }

    pub fn skill_proposals(&self) -> Result<Vec<Proposal>> {
        skills::proposals(&self.db)
    }

    pub fn accept_skill(
        &self,
        id: i64,
        name: &str,
        description: &str,
        body: &str,
    ) -> Result<String> {
        skills::accept(
            &self.db,
            &self.paths.skills_dir(),
            id,
            name,
            description,
            body,
        )
    }

    pub fn reject_skill(&self, id: i64) -> Result<()> {
        skills::reject(&self.db, id)
    }

    /// Après une tâche réussie : propose d'en tirer une compétence réutilisable.
    pub(crate) async fn propose_skill(&self, task_id: &str) -> Result<()> {
        let (task, steps) = self.db.get_task(task_id)?.context("tâche introuvable")?;
        let trace: String = steps
            .iter()
            .map(|s| {
                format!(
                    "## {} ({})\n{}\nJournal :\n{}",
                    s.title,
                    s.agent,
                    s.instructions,
                    clip(&s.log, 1500)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let schema = json!({
            "type": "object", "additionalProperties": false,
            "required": ["useful", "name", "description", "body"],
            "properties": {
                "useful": {"type": "boolean"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "body": {"type": "string"}
            }
        });
        let prompt = format!(
            "Une tâche vient de réussir.\n\nObjectif : {}\n\nDéroulé :\n{}\n\nRésultat :\n{}\n\n\
             Si cette réussite contient une méthode réutilisable pour des demandes similaires (et pas \
             seulement un résultat ponctuel), rédige une compétence : name (court, sans accents), \
             description (une phrase : quand l'utiliser), body (procédure en Markdown : étapes, outils \
             à employer, pièges à éviter). Sinon, useful = false. Réponds en JSON.",
            task.objective,
            clip(&trace, 8000),
            clip(task.result.as_deref().unwrap_or(""), 2000)
        );
        let messages = vec![
            ChatMessage::text(
                "system",
                "Tu capitalises le savoir-faire en procédures claires et générales.",
            ),
            ChatMessage::text("user", prompt),
        ];
        let decision = self.route(self.config().routing.mode, &messages, None)?;
        let noop = |_: StreamEvent| {};
        let approver = crate::tasks::DenyAll;
        let cancel = claudio_models::CancelToken::default();
        let lc = LoopCtx {
            scope: "",
            approver: &approver,
            emit: &noop,
            cancel: &cancel,
            max_steps: 1,
            thinking: Some(false),
            json_schema: Some(schema),
            local_model: None,
        };
        let run = self.answer(&decision, &messages, &[], &lc).await?;
        self.record_run(&run, None)?;
        let text = &run.content;
        let v: Value = serde_json::from_str(
            text.get(text.find('{').unwrap_or(0)..=text.rfind('}').unwrap_or(0))
                .unwrap_or("{}"),
        )
        .unwrap_or(Value::Null);
        if v["useful"].as_bool() != Some(true) {
            return Ok(());
        }
        let (name, desc, body) = (
            v["name"].as_str().unwrap_or(""),
            v["description"].as_str().unwrap_or(""),
            v["body"].as_str().unwrap_or(""),
        );
        if skills::slug(name).is_empty() || body.trim().len() < 40 {
            return Ok(());
        }
        let id = skills::propose(&self.db, name, desc, body, &task.title)?;
        self.emit_event(AppEvent::SkillProposed {
            id,
            name: skills::slug(name),
        });
        Ok(())
    }
}
