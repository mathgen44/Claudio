// Implémentation simulée des commandes, pour développer l'interface dans un
// navigateur sans le cœur Rust (`npm run dev`). Jamais utilisée dans l'application.

import type {
  KnowledgeStatus,
  Memory,
  Skill,
  SkillProposal,
  AccessOverview,
  AgentDef,
  AppEvent,
  ScheduleView,
  TaskDetail,
  TaskStep,
  ActiveModel,
  Answer,
  AuditEntry,
  Config,
  HfRepo,
  ProviderView,
  StreamEvent,
  UsageRow,
  InstallEvent,
  InstalledModel,
  Message,
  Progress,
  Recommendation,
  Session,
  Status,
  VoiceStatus,
} from "./api";

const GB = 1024 ** 3;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** `?ready` : machine déjà configurée (mise en route faite, un modèle installé). */
const READY = new URLSearchParams(location.search).has("ready");

const config: Config = {
  models_dir: null,
  active_model: READY ? { source: "local", id: "gemma-4-12b:Q6_K" } : null,
  context_size: null,
  use_ollama: true,
  llama_backend: null,
  thinking: null,
  system_prompt: "Tu es Claudio, un assistant personnel qui fonctionne sur l'ordinateur de l'utilisateur.",
  onboarding_done: READY,
  providers: [],
  routing: { mode: "auto", cloud: null },
  budget: { daily_usd: 2, monthly_usd: 20 },
  permissions: { network: "ask", shell: "ask", shell_sandbox: true, mcp: "ask", allow_private_network: false },
  mcp_servers: [],
  tools_enabled: true,
  memory: { auto_extract: true, recall: true, learn_skills: true },
  voice: {
    language: "fr",
    stt_model: null,
    wake_word: false,
    wake_phrase: "Claudio",
    speak: "voice",
    tts_engine: "system",
    system_voice: null,
    piper_voice: "piper-siwis",
    rate: 0,
  },
  telegram: { enabled: false, chats: [] },
  check_updates: true,
};

const MB = 1024 ** 2;
const voiceMock: VoiceStatus = {
  components: [
    { id: "whisper-runtime", label: "Moteur whisper.cpp", description: "Reconnaissance vocale locale (processeur), version b5454.", kind: "sttruntime", size: 9 * MB, installed: false },
    { id: "stt-base", label: "Whisper Base", description: "Très rapide : sert au mot d'éveil, ou à la dictée sur les petites machines.", kind: "stt", size: 57 * MB, installed: false },
    { id: "stt-small", label: "Whisper Small", description: "Bon compromis vitesse et qualité.", kind: "stt", size: 181 * MB, installed: false },
    { id: "stt-turbo", label: "Whisper Large v3 Turbo", description: "Meilleure transcription, plus lente sur les petits processeurs.", kind: "stt", size: 547 * MB, installed: false },
    { id: "piper-runtime", label: "Moteur Piper", description: "Synthèse vocale neuronale locale, plus naturelle que les voix Windows.", kind: "ttsruntime", size: 21 * MB, installed: false },
    { id: "piper-siwis", label: "Siwis (voix féminine)", description: "Voix Piper française.", kind: "voice", size: 60 * MB, installed: false },
    { id: "piper-tom", label: "Tom (voix masculine)", description: "Voix Piper française.", kind: "voice", size: 60 * MB, installed: false },
  ],
  stt_model: "stt-turbo",
  dictation_missing: [],
  wake_missing: [],
};
const telegramMock = { has_token: false, bot: null as string | null, error: null, pairing_code: null as string | null };

function refreshVoiceMock() {
  const missing = (ids: string[]) => ids.filter((id) => !voiceMock.components.find((c) => c.id === id)?.installed);
  voiceMock.stt_model = config.voice.stt_model ?? "stt-turbo";
  voiceMock.dictation_missing = missing(["whisper-runtime", voiceMock.stt_model]);
  voiceMock.wake_missing = missing(["whisper-runtime", "stt-base"]);
}

const memoriesMock: Memory[] = [
  { id: 1, content: "L'utilisateur préfère des réponses courtes.", category: "préférence", source: "manuel", session_id: null, pinned: true, created_at: "2026-10-20T10:00:00Z", updated_at: "2026-10-20T10:00:00Z" },
  { id: 2, content: "L'utilisateur fait du vélo tous les dimanches avec son club.", category: "personnel", source: "auto", session_id: null, pinned: false, created_at: "2026-10-25T10:00:00Z", updated_at: "2026-10-25T10:00:00Z" },
];
const knowledgeMock: KnowledgeStatus = {
  embedding_model: "Qwen3 Embedding 0.6B",
  recommended: ["qwen3-embedding-0.6b", "Q8_0"],
  pending_embeddings: 0,
  sources: [{ id: 1, path: "D:\\Documents\\Maison", enabled: true, status: "idle", error: null, last_scan: "2026-10-26T15:00:00Z", files: 128, failed_files: 2, chunks: 942, embedded_chunks: 942 }],
};
const skillsMock: Skill[] = [{ name: "revue-de-presse", description: "Faire une revue de presse sur un sujet.", body: "1. Chercher avec web_search\n2. Lire 3 à 5 articles\n3. Résumer avec les sources" }];
const proposalsMock: SkillProposal[] = [{ id: 1, name: "liste-de-courses", description: "Préparer une liste de courses classée par rayon à partir de menus.", body: "## Étapes\n1. Définir les menus\n2. Extraire les ingrédients\n3. Classer par rayon", origin: "Liste de courses végétarienne", created_at: "2026-10-26T15:00:00Z" }];

const access: AccessOverview = {
  workspace: "C:\\Users\\vous\\AppData\\Local\\Claudio\\workspace",
  grants: [{ id: 1, kind: "folder", target: "D:\\Documents\\Factures", write: false, created_at: "" }],
  permissions: config.permissions,
  tools_enabled: true,
  mcp_servers: [],
};
const audit: AuditEntry[] = [];
const pendingApprovals = new Map<string, (a: Answer) => void>();
let approvalSeq = 0;
const logAudit = (tool: string, detail: string, decision: string, outcome = "ok") =>
  audit.unshift({ id: audit.length + 1, at: new Date().toISOString(), session_id: null, tool, detail, decision, outcome });

const providers: ProviderView[] = [];
const usage: UsageRow[] = [
  { provider: "local", model: "gemma-4-12b:Q6_K", local: true, requests: 42, input_tokens: 51200, output_tokens: 18300, cost_usd: 0, unpriced: 0 },
];
let spentToday = 0;

const hfRepo: HfRepo = {
  id: "unsloth/Qwen3.6-35B-A3B-GGUF",
  url: "https://huggingface.co/unsloth/Qwen3.6-35B-A3B-GGUF",
  license: "apache-2.0",
  license_needs_review: false,
  gated: false,
  downloads: 1292945,
  likes: 812,
  variants: (
    [
      ["UD-Q3_K_M", 16.6, "hybrid"],
      ["UD-IQ4_XS", 17.7, "hybrid"],
      ["UD-Q4_K_M", 22.1, "hybrid"],
      ["Q8_0", 36.9, "hybrid"],
      ["BF16", 69.4, "toobig"],
    ] as const
  ).map(([quant, gb, placement]) => ({
    quant,
    files: [{ path: `Qwen3.6-35B-A3B-${quant}.gguf`, size: gb * 1e9, sha256: null }],
    size: gb * 1e9,
    fit: { params_b: 34.7, memory_needed: gb * 1e9 + 1.2e9, placement },
    installed: false,
  })),
};

const recos: Recommendation[] = [
  ["gemma-4-12b", "Gemma 4 12B", "Q6_K", 9.8, 11.9, 62.4, "Excellent en français et en rédaction, pour les cartes de 10 à 16 Go."],
  ["gemma-4-12b", "Gemma 4 12B", "Q5_K_M", 8.4, 10.6, 61.7, "Excellent en français et en rédaction, pour les cartes de 10 à 16 Go."],
  ["qwen3.5-9b", "Qwen 3.5 9B", "Q8_0", 9.5, 10.7, 58.0, "Polyvalent et solide, idéal pour les cartes de 8 Go et plus."],
  ["qwen3.5-4b", "Qwen 3.5 4B", "Q8_0", 4.5, 5.6, 45.0, "Bon compromis pour les cartes graphiques de 4 à 6 Go."],
  ["qwen3.5-0.8b", "Qwen 3.5 0.8B", "Q4_K_M", 0.53, 1.5, 19.0, "Très petit et très rapide."],
].map(([model_id, model_name, quant, size, need, score, description]) => ({
  model_id: model_id as string,
  model_name: model_name as string,
  quant: quant as string,
  file_size: (size as number) * 1e9,
  placement: "gpu" as const,
  context: 8192,
  memory_needed: (need as number) * 1e9,
  score: score as number,
  installed: false,
  description: description as string,
  license: "apache-2.0",
}));

const installed: InstalledModel[] = READY
  ? [{ key: "gemma-4-12b:Q6_K", model_id: "gemma-4-12b", name: "Gemma 4 12B", quant: "Q6_K", repo: "unsloth/gemma-4-12b-it-GGUF", file: "…", size: 9.8e9, sha256: null, installed_at: "" }]
  : [];
let runtime: Status["runtime"] = null;
const sessions: Session[] = [];
const messages: Message[] = [];
let nextId = 1;
const cancelled = new Set<string>();

function status(): Status {
  return {
    hardware: {
      cpu: { name: "AMD Ryzen 7 9800X3D 8-Core Processor", physical_cores: 8, logical_cores: 16, avx2: true, avx512: true },
      ram_total: 64 * GB,
      ram_available: 38 * GB,
      gpus: [
        {
          name: "NVIDIA GeForce RTX 5070 Ti",
          vendor: "nvidia",
          vram_total: 16 * GB,
          vram_free: 12.6 * GB,
          driver_version: "617.14",
          cuda_version: [13, 4],
          compute_capability: [12, 0],
          integrated: false,
        },
        {
          name: "AMD Radeon(TM) Graphics",
          vendor: "amd",
          vram_total: 2 * GB,
          vram_free: 2 * GB,
          driver_version: null,
          cuda_version: null,
          compute_capability: null,
          integrated: true,
        },
      ],
      os: "Windows 11 Pro",
    },
    profile: { tier: "L", gpu_index: 0, vram_budget: 11.8 * GB, ram_budget: 34 * GB, max_parallel_agents: 3, threads: 8 },
    runtime,
    active_model: config.active_model,
    loaded_model: null,
    ollama_models: [{ name: "llama3.2:3b", size: 2e9, details: { parameter_size: "3.2B", quantization_level: "Q4_K_M" } }],
    installed_models: installed,
    has_hf_token: false,
    embedding_models: [],
    onboarding_done: config.onboarding_done,
    models_dir: "C:\\Users\\vous\\AppData\\Local\\Claudio\\models",
  };
}

async function fakeDownload(total: number, key: string, report: (p: Progress) => void) {
  const steps = 25;
  for (let i = 1; i <= steps; i++) {
    if (cancelled.delete(key)) throw "téléchargement annulé";
    await sleep(120);
    report({ downloaded: (total * i) / steps, total, bytes_per_sec: 85e6 });
  }
}

// ---- Tâches simulées ----------------------------------------------------------

const listeners = new Set<(e: AppEvent) => void>();
export function mockListen(handler: (e: AppEvent) => void): () => void {
  listeners.add(handler);
  return () => listeners.delete(handler);
}
const emitApp = (e: AppEvent) => listeners.forEach((l) => l(e));

const agent = (id: string, name: string, description: string, tools: string[]): AgentDef => ({
  id, name, description, tools, model: "auto", max_steps: 12, thinking: null, system_prompt: "…", builtin: true,
});
const AGENTS: AgentDef[] = [
  agent("assistant", "Assistant", "Généraliste polyvalent.", ["*"]),
  agent("planner", "Planificateur", "Découpe un objectif en étapes.", []),
  agent("researcher", "Documentaliste", "Recherche d'informations sur le web et dans les fichiers, avec sources.", ["web_search", "web_fetch", "read_file"]),
  agent("analyst", "Analyste", "Analyse des données et présente des chiffres.", ["read_file", "run_powershell"]),
  agent("reviewer", "Relecteur", "Vérifie les résultats et rédige la réponse finale.", ["read_file"]),
];
const tasks: TaskDetail[] = [];
const schedules: ScheduleView[] = [];

function simulateTask(t: TaskDetail) {
  const step = (step_id: string, agent: string, title: string, depends_on: string[]): TaskStep => ({
    step_id, position: 0, agent, title, instructions: `Mission : ${title.toLowerCase()}.`, depends_on, status: "pending",
    output: null, error: null, model: null, log: "", started_at: null, finished_at: null,
  });
  const upd = () => emitApp({ type: "task_updated", task_id: t.id });
  setTimeout(() => {
    t.steps = [
      step("e1", "researcher", "Rechercher les tarifs", []),
      step("e2", "analyst", "Analyser les factures", []),
      step("synthese", "reviewer", "Vérification et synthèse", ["e1", "e2"]),
    ];
    t.levels = [["e1", "e2"], ["synthese"]];
    t.steps_total = 3;
    t.status = "running";
    upd();
    const run = async (s: TaskStep, text: string, ms: number) => {
      s.status = "running";
      s.model = "gemma-4-12b:Q6_K";
      s.log += "▶ local · gemma-4-12b:Q6_K · modèle local\n🔧 read_file {\"path\":\"D:\\\\Factures\\\\2026-09.pdf\"}\n";
      upd();
      for (const w of text.split(/(?<= )/)) {
        await sleep(ms);
        emitApp({ type: "step_output", task_id: t.id, step_id: s.step_id, text: w });
      }
      s.status = "done";
      s.output = text;
      t.steps_done += 1;
      upd();
    };
    Promise.all([
      run(t.steps[0], "Le tarif de base EDF est de **0,2516 €/kWh** (source : edf.fr).", 60),
      run(t.steps[1], "Consommation moyenne : **412 kWh/mois**, en hausse de 8 % sur un an.", 80),
    ]).then(async () => {
      await run(t.steps[2], "## Synthèse\n\nVotre facture moyenne est d'environ **104 €/mois**. Pistes : heures creuses, chauffe-eau programmé.", 40);
      t.status = "done";
      t.result = t.steps[2].output;
      t.tokens = 4210;
      upd();
      emitApp({ type: "task_finished", task_id: t.id, title: t.title, status: "done" });
    });
  }, 900);
}

export async function mockInvoke<T>(cmd: string, args: Record<string, any> = {}): Promise<T> {
  await sleep(60);
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "get_status":
      return r(status());
    case "refresh_hardware":
    case "warm_up":
      await sleep(400);
      return r(undefined);
    case "get_recommendations":
      return r(recos.map((x) => ({ ...x, installed: installed.some((i) => i.key === `${x.model_id}:${x.quant}`) })));
    case "install_runtime": {
      const on = args.onEvent as (e: InstallEvent) => void;
      on({ stage: "resolving" });
      await fakeDownload(165e6, "runtime", (progress) => on({ stage: "downloading", file: "llama-bin-win-cuda-13.4-x64.zip", progress }));
      on({ stage: "extracting" });
      await sleep(500);
      runtime = { tag: "b11896", backend: { kind: "cuda", version: "13.4" }, dir: "C:\\…\\runtimes" };
      return r(runtime);
    }
    case "install_model": {
      const reco = recos.find((x) => x.model_id === args.modelId && x.quant === args.quant)!;
      const key = `${reco.model_id}:${reco.quant}`;
      await fakeDownload(reco.file_size, key, args.onProgress);
      const m: InstalledModel = {
        key,
        model_id: reco.model_id,
        name: reco.model_name,
        quant: reco.quant,
        repo: "unsloth/…",
        file: "…",
        size: reco.file_size,
        sha256: null,
        installed_at: new Date().toISOString(),
      };
      installed.push(m);
      if (!config.active_model) config.active_model = { source: "local", id: key };
      return r(m);
    }
    case "cancel_download":
      cancelled.add(args.key);
      return r(undefined);
    case "remove_model": {
      const i = installed.findIndex((m) => m.key === args.key);
      if (i >= 0) installed.splice(i, 1);
      return r(undefined);
    }
    case "set_active_model":
      config.active_model = args.model as ActiveModel;
      return r(undefined);
    case "get_config":
      return r({ ...config });
    case "update_settings": {
      const p = args.patch;
      if ("contextSize" in p) config.context_size = p.contextSize;
      if ("useOllama" in p) config.use_ollama = p.useOllama;
      if ("systemPrompt" in p) config.system_prompt = p.systemPrompt;
      if ("llamaBackend" in p) config.llama_backend = p.llamaBackend;
      if ("onboardingDone" in p) config.onboarding_done = p.onboardingDone;
      if ("thinking" in p) config.thinking = p.thinking;
      if ("checkUpdates" in p) config.check_updates = p.checkUpdates;
      return r({ ...config });
    }
    case "set_hf_token":
    case "open_folder":
    case "cancel_chat":
      return r(undefined);
    case "list_sessions":
      return r([...sessions].sort((a, b) => b.updated_at.localeCompare(a.updated_at)));
    case "create_session": {
      const now = new Date().toISOString();
      const s: Session = { id: crypto.randomUUID(), title: "Nouvelle conversation", route: null, created_at: now, updated_at: now };
      sessions.push(s);
      return r(s);
    }
    case "rename_session":
      sessions.find((s) => s.id === args.id)!.title = args.title;
      return r(undefined);
    case "delete_session":
      sessions.splice(sessions.findIndex((s) => s.id === args.id), 1);
      return r(undefined);
    case "list_messages":
      return r(messages.filter((m) => m.session_id === args.sessionId));
    case "send_message": {
      const on = args.onEvent as (e: StreamEvent) => void;
      const s = sessions.find((x) => x.id === args.sessionId)!;
      const now = () => new Date().toISOString();
      messages.push({ id: nextId++, session_id: s.id, role: "user", content: args.text, model: null, tokens: null, duration_ms: null, reasoning: null, route: null, cost_usd: null, created_at: now() });
      const mode = s.route ?? config.routing.mode;
      const cloud = mode === "cloud" && config.routing.cloud;
      const route = cloud ? "mode cloud" : "modèle local";
      const model = cloud ? config.routing.cloud!.model : "gemma-4-12b:Q6_K";
      on({ type: "route", target: cloud ? "cloud" : "local", model, reason: route });
      // Démonstration d'un appel d'outil avec demande d'autorisation.
      const dir = "D:\\Documents\\Projets";
      const toolArgs = JSON.stringify({ path: dir });
      on({ type: "tool_call", id: "call_1", name: "list_directory", arguments: toolArgs });
      const id = `ap-${++approvalSeq}`;
      const decision = await new Promise<Answer>((resolve) => {
        pendingApprovals.set(id, resolve);
        on({ type: "approval", id, tool: "list_directory", title: "Lire des fichiers", detail: dir, options: ["once", "session", "always"], dangerous: false });
      });
      const ok = decision !== "deny";
      const result = ok ? "[dossier] Claudio\\\n[dossier] Site web\\\nnotes.md  2 Ko  2026-10-20 18:02" : "refusé par l'utilisateur";
      logAudit("list_directory", dir, ok ? `approuvé (${decision})` : "refusé : refusé par l'utilisateur", ok ? "ok" : result);
      on({ type: "tool_result", id: "call_1", name: "list_directory", content: result, is_error: !ok });
      messages.push({
        id: nextId++, session_id: s.id, role: "tool",
        content: JSON.stringify({ name: "list_directory", arguments: toolArgs, result, is_error: !ok }),
        model: null, tokens: null, duration_ms: null, reasoning: null, route: null, cost_usd: null, created_at: now(),
      });
      if (s.title === "Nouvelle conversation") s.title = String(args.text).slice(0, 48);
      const answer =
        "Bonjour ! Je suis **Claudio**, ton assistant local.\n\nJe tourne entièrement sur ta machine :\n\n- modèle chargé sur la carte graphique\n- aucune donnée envoyée sur Internet\n\n```powershell\nGet-Process llama-server\n```";
      for (const w of "L'utilisateur me salue, je me présente.".split(/(?<= )/)) {
        on({ type: "reasoning", text: w });
        await sleep(25);
      }
      for (const w of answer.split(/(?<= )/)) {
        on({ type: "token", text: w });
        await sleep(25);
      }
      const cost = cloud ? 0.0123 : null;
      if (cost) spentToday += cost;
      const msg: Message = {
        id: nextId++,
        session_id: s.id,
        role: "assistant",
        content: answer,
        model: cloud ? `${model} · Anthropic` : model,
        tokens: 64,
        duration_ms: 900,
        reasoning: "L'utilisateur me salue, je me présente.",
        route,
        cost_usd: cost,
        created_at: now(),
      };
      messages.push(msg);
      s.updated_at = now();
      return r({ message: msg, tokens_per_sec: 88.4, cancelled: false });
    }
    case "answer_approval":
      pendingApprovals.get(args.id)?.(args.answer);
      pendingApprovals.delete(args.id);
      return r(undefined);
    case "access_overview":
      return r({ ...access, permissions: config.permissions, tools_enabled: config.tools_enabled, mcp_servers: config.mcp_servers });
    case "grant_folder":
      access.grants.push({ id: Date.now(), kind: "folder", target: args.path, write: args.write, created_at: "" });
      return r(undefined);
    case "revoke_grant":
      access.grants = access.grants.filter((g) => g.id !== args.id);
      return r(undefined);
    case "set_permissions":
      config.permissions = args.permissions;
      return r(undefined);
    case "set_tools_enabled":
      config.tools_enabled = args.enabled;
      return r(undefined);
    case "audit_log":
      return r(audit.filter((e) => args.before == null || e.id < args.before).slice(0, args.limit));
    case "verify_audit":
      return r(null);
    case "add_mcp_server": {
      const srv = { id: `mcp${config.mcp_servers.length + 1}`, name: args.name || "Serveur MCP", transport: args.transport, enabled: true };
      config.mcp_servers.push(srv);
      return r(srv);
    }
    case "set_mcp_enabled":
      config.mcp_servers.find((m) => m.id === args.id)!.enabled = args.enabled;
      return r(undefined);
    case "remove_mcp_server":
      config.mcp_servers = config.mcp_servers.filter((m) => m.id !== args.id);
      return r(undefined);
    case "test_mcp_server":
      await sleep(500);
      return r([
        { name: "search_repositories", description: "Cherche des dépôts" },
        { name: "create_issue", description: "Crée un ticket" },
      ]);
    case "list_agents":
      return r({ agents: AGENTS, errors: [] });
    case "list_tasks":
      return r(tasks.map(({ steps: _s, levels: _l, ...t }) => t));
    case "task_detail":
      return r(structuredClone(tasks.find((t) => t.id === args.id)!));
    case "start_task": {
      const now = new Date().toISOString();
      const t: TaskDetail = {
        id: crypto.randomUUID(), title: String(args.objective).slice(0, 48), objective: args.objective, mode: args.mode,
        status: "planning", result: null, error: null, schedule_id: null, tokens: 0, cost_usd: 0, created_at: now,
        updated_at: now, steps_done: 0, steps_total: 0, steps: [], levels: [],
      };
      tasks.unshift(t);
      simulateTask(t);
      return r(t);
    }
    case "cancel_task":
    case "resume_task":
      return r(undefined);
    case "delete_task":
      tasks.splice(tasks.findIndex((t) => t.id === args.id), 1);
      return r(undefined);
    case "list_schedules":
      return r(schedules);
    case "add_schedule": {
      const s: ScheduleView = {
        id: schedules.length + 1, title: args.title || String(args.objective).slice(0, 48), objective: args.objective,
        mode: args.mode, cron: args.cron, enabled: true, last_run: null, created_at: new Date().toISOString(),
        next_run: new Date(Date.now() + 36e5 * 14).toISOString(),
      };
      schedules.push(s);
      return r(s);
    }
    case "set_schedule_enabled":
      schedules.find((s) => s.id === args.id)!.enabled = args.enabled;
      return r(undefined);
    case "delete_schedule":
      schedules.splice(schedules.findIndex((s) => s.id === args.id), 1);
      return r(undefined);
    case "run_schedule_now":
      return r("x");
    case "knowledge_status":
      return r(knowledgeMock);
    case "add_index_source":
      knowledgeMock.sources.push({ id: knowledgeMock.sources.length + 1, path: args.path, enabled: true, status: "idle", error: null, last_scan: new Date().toISOString(), files: 12, failed_files: 0, chunks: 40, embedded_chunks: 40 });
      return r(knowledgeMock.sources.length);
    case "remove_index_source":
      knowledgeMock.sources = knowledgeMock.sources.filter((s) => s.id !== args.id);
      return r(undefined);
    case "reindex_source":
    case "reindex_all":
    case "set_memory_settings":
      return r(undefined);
    case "telegram_status":
      return r({ ...telegramMock, enabled: config.telegram.enabled, chats: config.telegram.chats, running: config.telegram.enabled && telegramMock.has_token });
    case "set_telegram_token":
      telegramMock.has_token = !!args.token;
      telegramMock.bot = args.token ? "@claudio_maison_bot" : null;
      return r(undefined);
    case "set_telegram_enabled":
      config.telegram.enabled = args.enabled;
      return r(undefined);
    case "telegram_pairing_code":
      telegramMock.pairing_code = "482913";
      setTimeout(() => {
        config.telegram.chats.push({ id: 1, name: "Hervé (@herve)" });
        telegramMock.pairing_code = null;
      }, 4000);
      return r("482913");
    case "remove_telegram_chat":
      config.telegram.chats = config.telegram.chats.filter((c) => c.id !== args.id);
      return r(undefined);
    case "voice_status":
      refreshVoiceMock();
      return r(structuredClone(voiceMock));
    case "install_voice": {
      const c = voiceMock.components.find((x) => x.id === args.id)!;
      await fakeDownload(c.size, `voice:${c.id}`, args.onProgress);
      c.installed = true;
      return r(undefined);
    }
    case "remove_voice":
      voiceMock.components.find((x) => x.id === args.id)!.installed = false;
      return r(undefined);
    case "system_voices":
      return r([
        { name: "Microsoft Hortense Desktop", culture: "fr-FR" },
        { name: "Microsoft Zira Desktop", culture: "en-US" },
      ]);
    case "set_voice_settings":
      config.voice = args.settings;
      return r(undefined);
    case "transcribe":
      await sleep(500);
      return r({ text: args.wake ? "Claudio" : "Quelle est la météo prévue demain à Nantes ?", wake: args.wake ? "" : null, seconds: 3 });
    case "search_documents":
      return r([{ path: "D:\\Documents\\Maison\\factures\\edf-septembre.pdf", location: "page 1", text: "Facture EDF - septembre 2026\nMontant TTC : 104,20 euros", score: 0.03 }]);
    case "list_memories":
      return r(memoriesMock.filter((m) => !args.filter || m.content.toLowerCase().includes(String(args.filter).toLowerCase())));
    case "add_memory": {
      const now = new Date().toISOString();
      const m: Memory = { id: Date.now(), content: args.content, category: args.category, source: "manuel", session_id: null, pinned: false, created_at: now, updated_at: now };
      memoriesMock.unshift(m);
      return r(m);
    }
    case "update_memory": {
      const m = memoriesMock.find((x) => x.id === args.id)!;
      Object.assign(m, { content: args.content, category: args.category, pinned: args.pinned });
      return r(m);
    }
    case "delete_memory":
      memoriesMock.splice(memoriesMock.findIndex((m) => m.id === args.id), 1);
      return r(undefined);
    case "delete_all_memories":
      return r(memoriesMock.splice(0).length);
    case "list_skills":
      return r(skillsMock);
    case "save_skill":
      skillsMock.push({ name: args.name, description: args.description, body: args.body });
      return r(args.name);
    case "delete_skill":
      skillsMock.splice(skillsMock.findIndex((s) => s.name === args.name), 1);
      return r(undefined);
    case "skill_proposals":
      return r(proposalsMock);
    case "accept_skill":
      skillsMock.push({ name: args.name, description: args.description, body: args.body });
      proposalsMock.splice(0);
      return r(args.name);
    case "reject_skill":
      proposalsMock.splice(0);
      return r(undefined);
    case "set_session_route":
      sessions.find((s) => s.id === args.sessionId)!.route = args.mode;
      return r(undefined);
    case "hf_search":
      return r(
        ["unsloth/Qwen3.6-35B-A3B-GGUF", "unsloth/gemma-4-12b-it-GGUF", "bartowski/Mistral-Small-3.3-24B-GGUF"].map((id, i) => ({
          id,
          downloads: 1_300_000 / (i + 1),
          likes: 800 / (i + 1),
          license: i === 2 ? "mrl" : "apache-2.0",
          pipeline_tag: "text-generation",
          updated: null,
        })),
      );
    case "hf_repo":
      return r({ ...hfRepo, id: args.repo, license_needs_review: args.repo.includes("Mistral"), license: args.repo.includes("Mistral") ? "mrl" : "apache-2.0" });
    case "install_hf": {
      const v = hfRepo.variants.find((x) => x.quant === args.quant)!;
      await fakeDownload(v.size, `${args.repo}:${args.quant}`, args.onProgress);
      v.installed = true;
      return r({ key: `${args.repo}:${args.quant}`, model_id: args.repo, name: "Qwen3.6-35B-A3B", quant: args.quant, repo: args.repo, file: "…", size: v.size, sha256: null, installed_at: new Date().toISOString() });
    }
    case "list_providers":
      return r(providers);
    case "add_provider": {
      const p: ProviderView = {
        id: `${args.kind}-${providers.length + 1}`,
        kind: args.kind,
        name: args.name || "Anthropic",
        base_url: args.baseUrl,
        enabled: true,
        prices: [],
        has_key: !!args.apiKey,
        kind_label: "Anthropic (Claude)",
        default_base_url: "https://api.anthropic.com",
      };
      providers.push(p);
      return r(p);
    }
    case "update_provider": {
      const p = providers.find((x) => x.id === args.id)!;
      if (args.enabled != null) p.enabled = args.enabled;
      return r(undefined);
    }
    case "set_provider_key":
      providers.find((x) => x.id === args.id)!.has_key = !!args.key;
      return r(undefined);
    case "remove_provider":
      providers.splice(providers.findIndex((x) => x.id === args.id), 1);
      if (config.routing.cloud?.provider_id === args.id) config.routing.cloud = null;
      return r(undefined);
    case "provider_models":
      await sleep(300);
      return r([
        { id: "claude-opus-5-5", name: "Claude Opus 5.5", input_per_m: 4, output_per_m: 20 },
        { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5", input_per_m: 2, output_per_m: 10 },
        { id: "claude-haiku-5-5", name: "Claude Haiku 5.5", input_per_m: 0.1, output_per_m: 0.5 },
      ]);
    case "set_model_price":
      return r(undefined);
    case "set_cloud_model":
      config.routing.cloud = args.model;
      return r(undefined);
    case "set_routing_mode":
      config.routing.mode = args.mode;
      return r(undefined);
    case "set_budget":
      config.budget = args.budget;
      return r(undefined);
    case "budget_status":
    case "usage_report": {
      const budget = {
        today_usd: spentToday + 0.42,
        month_usd: spentToday + 6.8,
        daily_usd: config.budget.daily_usd,
        monthly_usd: config.budget.monthly_usd,
        exceeded: false,
        warning: false,
      };
      if (cmd === "budget_status") return r(budget);
      const month = [
        ...usage,
        { provider: "anthropic-1", model: "claude-opus-5-5", local: false, requests: 12, input_tokens: 240000, output_tokens: 61000, cost_usd: 2.18, unpriced: 0 },
        { provider: "openrouter-1", model: "mystery-model", local: false, requests: 3, input_tokens: 9000, output_tokens: 2000, cost_usd: 0, unpriced: 3 },
      ];
      const daily = Array.from({ length: 30 }, (_, i) => ({
        day: new Date(Date.now() - (29 - i) * 864e5).toISOString().slice(0, 10),
        cost_usd: i % 4 === 0 ? 0 : Math.abs(Math.sin(i)) * 0.6,
        local_requests: 5,
        cloud_requests: i % 4 === 0 ? 0 : 2,
      }));
      return r({ budget, month, daily });
    }
    default:
      throw `commande simulée inconnue : ${cmd}`;
  }
}
