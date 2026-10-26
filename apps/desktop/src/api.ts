// Accès aux commandes du cœur Rust. Hors de Tauri (aperçu dans un navigateur),
// une implémentation simulée prend le relais.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { mockInvoke, mockListen } from "./mock";

export type GpuVendor = "nvidia" | "amd" | "intel" | "other";
export type Tier = "S" | "M" | "L" | "XL";

export interface GpuInfo {
  name: string;
  vendor: GpuVendor;
  vram_total: number;
  vram_free: number | null;
  driver_version: string | null;
  cuda_version: [number, number] | null;
  compute_capability: [number, number] | null;
  integrated: boolean;
}

export interface HardwareInfo {
  cpu: { name: string; physical_cores: number; logical_cores: number; avx2: boolean; avx512: boolean };
  ram_total: number;
  ram_available: number;
  gpus: GpuInfo[];
  os: string;
}

export interface MachineProfile {
  tier: Tier;
  gpu_index: number | null;
  vram_budget: number;
  ram_budget: number;
  max_parallel_agents: number;
  threads: number;
}

export type Backend = { kind: "cuda"; version: string } | { kind: "vulkan" } | { kind: "cpu" };

export interface InstalledRuntime {
  tag: string;
  backend: Backend;
  dir: string;
}

export type ActiveModel = { source: "local"; id: string } | { source: "ollama"; name: string };

export interface OllamaModel {
  name: string;
  size: number;
  details: { parameter_size: string; quantization_level: string };
}

export interface InstalledModel {
  key: string;
  model_id: string;
  name: string;
  quant: string;
  repo: string;
  file: string;
  size: number;
  sha256: string | null;
  installed_at: string;
}

export interface Status {
  hardware: HardwareInfo;
  profile: MachineProfile;
  runtime: InstalledRuntime | null;
  active_model: ActiveModel | null;
  loaded_model: string | null;
  ollama_models: OllamaModel[] | null;
  installed_models: InstalledModel[];
  has_hf_token: boolean;
  onboarding_done: boolean;
  models_dir: string;
}

export interface Recommendation {
  model_id: string;
  model_name: string;
  quant: string;
  file_size: number;
  placement: Placement;
  context: number;
  memory_needed: number;
  score: number;
  installed: boolean;
  description: string;
  license: string;
}

export interface Progress {
  downloaded: number;
  total: number | null;
  bytes_per_sec: number;
}

export type InstallEvent =
  | { stage: "resolving" }
  | { stage: "downloading"; file: string; progress: Progress }
  | { stage: "extracting" };

export interface Config {
  models_dir: string | null;
  active_model: ActiveModel | null;
  context_size: number | null;
  use_ollama: boolean;
  llama_backend: string | null;
  thinking: boolean | null;
  system_prompt: string;
  onboarding_done: boolean;
  providers: ProviderConfig[];
  routing: { mode: RoutingMode; cloud: CloudModel | null };
  budget: Budget;
  permissions: Permissions;
  mcp_servers: McpServer[];
  tools_enabled: boolean;
}

export type Policy = "ask" | "allow" | "deny";

export interface Permissions {
  network: Policy;
  shell: Policy;
  shell_sandbox: boolean;
  mcp: Policy;
  allow_private_network: boolean;
}

export type McpTransport =
  | { type: "stdio"; command: string; args: string[]; env: [string, string][] }
  | { type: "http"; url: string };

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  enabled: boolean;
}

export interface McpTool {
  name: string;
  description: string;
}

export interface Grant {
  id: number;
  kind: "folder" | "host" | "mcp";
  target: string;
  write: boolean;
  created_at: string;
}

export interface AccessOverview {
  workspace: string;
  grants: Grant[];
  permissions: Permissions;
  tools_enabled: boolean;
  mcp_servers: McpServer[];
}

export interface AuditEntry {
  id: number;
  at: string;
  session_id: string | null;
  tool: string;
  detail: string;
  decision: string;
  outcome: string;
}

export type Answer = "deny" | "once" | "session" | "always";

export interface ApprovalRequest {
  id: string;
  tool: string;
  title: string;
  detail: string;
  options: Answer[];
  dangerous: boolean;
}

export type ModelPref = "auto" | "local" | "cloud";

export interface AgentDef {
  id: string;
  name: string;
  description: string;
  model: ModelPref;
  tools: string[];
  max_steps: number;
  thinking: boolean | null;
  system_prompt: string;
  builtin: boolean;
}

export interface AgentsView {
  agents: AgentDef[];
  errors: [string, string][];
}

export type TaskStatus = "planning" | "running" | "done" | "partial" | "failed" | "cancelled" | "interrupted";
export type StepStatus = "pending" | "running" | "waiting" | "done" | "failed" | "skipped" | "cancelled";

export interface Task {
  id: string;
  title: string;
  objective: string;
  mode: string;
  status: TaskStatus;
  result: string | null;
  error: string | null;
  schedule_id: number | null;
  tokens: number;
  cost_usd: number;
  created_at: string;
  updated_at: string;
  steps_done: number;
  steps_total: number;
}

export interface TaskStep {
  step_id: string;
  position: number;
  agent: string;
  title: string;
  instructions: string;
  depends_on: string[];
  status: StepStatus;
  output: string | null;
  error: string | null;
  model: string | null;
  log: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface TaskDetail extends Task {
  steps: TaskStep[];
  levels: string[][];
}

export interface ScheduleView {
  id: number;
  title: string;
  objective: string;
  mode: string;
  cron: string;
  enabled: boolean;
  last_run: string | null;
  created_at: string;
  next_run: string | null;
}

export type AppEvent =
  | { type: "task_updated"; task_id: string }
  | { type: "step_output"; task_id: string; step_id: string; text: string }
  | ({ type: "approval"; task_id: string; task_title: string; step_title: string } & ApprovalRequest)
  | { type: "task_finished"; task_id: string; title: string; status: TaskStatus };

export interface ToolTrace {
  name: string;
  arguments: string;
  result: string;
  is_error: boolean;
}

export type ProviderKind = "anthropic" | "openai" | "mistral" | "gemini" | "openrouter" | "custom";
export type RoutingMode = "local" | "auto" | "cloud";

export interface ModelPrice {
  model: string;
  input_per_m: number;
  output_per_m: number;
}

export interface ProviderConfig {
  id: string;
  kind: ProviderKind;
  name: string;
  base_url: string | null;
  enabled: boolean;
  prices: ModelPrice[];
}

export interface ProviderView extends ProviderConfig {
  has_key: boolean;
  kind_label: string;
  default_base_url: string | null;
}

export interface CloudModel {
  provider_id: string;
  model: string;
}

export interface Budget {
  daily_usd: number | null;
  monthly_usd: number | null;
}

export interface RemoteModel {
  id: string;
  name: string;
  input_per_m: number | null;
  output_per_m: number | null;
}

export interface BudgetStatus {
  today_usd: number;
  month_usd: number;
  daily_usd: number | null;
  monthly_usd: number | null;
  exceeded: boolean;
  warning: boolean;
}

export interface UsageRow {
  provider: string;
  model: string;
  local: boolean;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  unpriced: number;
}

export interface UsageReport {
  budget: BudgetStatus;
  month: UsageRow[];
  daily: { day: string; cost_usd: number; local_requests: number; cloud_requests: number }[];
}

export type Placement = "gpu" | "hybrid" | "cpu" | "toobig";

export interface HfSummary {
  id: string;
  downloads: number;
  likes: number;
  license: string | null;
  pipeline_tag: string | null;
  updated: string | null;
}

export interface HfVariant {
  quant: string;
  files: { path: string; size: number; sha256: string | null }[];
  size: number;
  fit: { params_b: number; memory_needed: number; placement: Placement };
  installed: boolean;
}

export interface HfRepo {
  id: string;
  url: string;
  license: string | null;
  license_needs_review: boolean;
  gated: boolean;
  downloads: number;
  likes: number;
  variants: HfVariant[];
}

export interface SettingsPatch {
  contextSize?: number | null;
  useOllama?: boolean;
  systemPrompt?: string;
  llamaBackend?: string | null;
  onboardingDone?: boolean;
  thinking?: boolean | null;
}

export interface Session {
  id: string;
  title: string;
  route: RoutingMode | null;
  created_at: string;
  updated_at: string;
}

export type Role = "system" | "user" | "assistant" | "tool";

export interface Message {
  id: number;
  session_id: string;
  role: Role;
  content: string;
  model: string | null;
  tokens: number | null;
  duration_ms: number | null;
  reasoning: string | null;
  route: string | null;
  cost_usd: number | null;
  created_at: string;
}

export type StreamEvent =
  | { type: "route"; target: "local" | "cloud"; model: string; reason: string }
  | { type: "token"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "tool_call"; id: string; name: string; arguments: string }
  | ({ type: "approval" } & ApprovalRequest)
  | { type: "tool_result"; id: string; name: string; content: string; is_error: boolean };

export interface Reply {
  message: Message;
  tokens_per_sec: number | null;
  cancelled: boolean;
}

export const inTauri = "__TAURI_INTERNALS__" in window;

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(cmd, args) : mockInvoke<T>(cmd, args);
}

/** Canal de flux : un vrai `Channel` Tauri, ou une simple fonction en mode simulé. */
function channel<T>(onMessage: (m: T) => void): Channel<T> | ((m: T) => void) {
  if (!inTauri) return onMessage;
  const ch = new Channel<T>();
  ch.onmessage = onMessage;
  return ch;
}

export const api = {
  status: () => call<Status>("get_status"),
  refreshHardware: () => call<void>("refresh_hardware"),
  recommendations: () => call<Recommendation[]>("get_recommendations"),
  installRuntime: (onEvent: (e: InstallEvent) => void) =>
    call<InstalledRuntime>("install_runtime", { onEvent: channel(onEvent) }),
  installModel: (modelId: string, quant: string, onProgress: (p: Progress) => void) =>
    call<InstalledModel>("install_model", { modelId, quant, onProgress: channel(onProgress) }),
  cancelDownload: (key: string) => call<void>("cancel_download", { key }),
  removeModel: (key: string) => call<void>("remove_model", { key }),
  setActiveModel: (model: ActiveModel) => call<void>("set_active_model", { model }),
  warmUp: () => call<void>("warm_up"),
  config: () => call<Config>("get_config"),
  updateSettings: (patch: SettingsPatch) => call<Config>("update_settings", { patch }),
  setHfToken: (token: string | null) => call<void>("set_hf_token", { token }),
  openFolder: (which: "logs" | "models" | "data" | "workspace" | "agents") => call<void>("open_folder", { which }),
  sessions: () => call<Session[]>("list_sessions"),
  createSession: () => call<Session>("create_session"),
  renameSession: (id: string, title: string) => call<void>("rename_session", { id, title }),
  deleteSession: (id: string) => call<void>("delete_session", { id }),
  messages: (sessionId: string) => call<Message[]>("list_messages", { sessionId }),
  sendMessage: (sessionId: string, text: string, onEvent: (e: StreamEvent) => void) =>
    call<Reply>("send_message", { sessionId, text, onEvent: channel(onEvent) }),
  cancelChat: (sessionId: string) => call<void>("cancel_chat", { sessionId }),
  setSessionRoute: (sessionId: string, mode: RoutingMode | null) =>
    call<void>("set_session_route", { sessionId, mode }),

  hfSearch: (query: string) => call<HfSummary[]>("hf_search", { query }),
  hfRepo: (repo: string) => call<HfRepo>("hf_repo", { repo }),
  installHf: (repo: string, quant: string, onProgress: (p: Progress) => void) =>
    call<InstalledModel>("install_hf", { repo, quant, onProgress: channel(onProgress) }),

  providers: () => call<ProviderView[]>("list_providers"),
  addProvider: (kind: ProviderKind, name: string, baseUrl: string | null, apiKey: string | null) =>
    call<ProviderView>("add_provider", { kind, name, baseUrl, apiKey }),
  updateProvider: (id: string, patch: { name?: string; baseUrl?: string; enabled?: boolean }) =>
    call<void>("update_provider", { id, ...patch }),
  setProviderKey: (id: string, key: string | null) => call<void>("set_provider_key", { id, key }),
  removeProvider: (id: string) => call<void>("remove_provider", { id }),
  providerModels: (id: string) => call<RemoteModel[]>("provider_models", { id }),
  setModelPrice: (providerId: string, model: string, input: number, output: number) =>
    call<void>("set_model_price", { providerId, model, input, output }),
  setCloudModel: (model: CloudModel | null, price: [number, number] | null) =>
    call<void>("set_cloud_model", { model, price }),
  setRoutingMode: (mode: RoutingMode) => call<void>("set_routing_mode", { mode }),
  setBudget: (budget: Budget) => call<void>("set_budget", { budget }),
  budgetStatus: () => call<BudgetStatus>("budget_status"),
  usageReport: () => call<UsageReport>("usage_report"),

  answerApproval: (id: string, answer: Answer) => call<void>("answer_approval", { id, answer }),
  accessOverview: () => call<AccessOverview>("access_overview"),
  grantFolder: (path: string, write: boolean) => call<void>("grant_folder", { path, write }),
  revokeGrant: (id: number) => call<void>("revoke_grant", { id }),
  setPermissions: (permissions: Permissions) => call<void>("set_permissions", { permissions }),
  setToolsEnabled: (enabled: boolean) => call<void>("set_tools_enabled", { enabled }),
  auditLog: (limit: number, before: number | null) => call<AuditEntry[]>("audit_log", { limit, before }),
  verifyAudit: () => call<number | null>("verify_audit"),
  addMcpServer: (name: string, transport: McpTransport) => call<McpServer>("add_mcp_server", { name, transport }),
  setMcpEnabled: (id: string, enabled: boolean) => call<void>("set_mcp_enabled", { id, enabled }),
  removeMcpServer: (id: string) => call<void>("remove_mcp_server", { id }),
  testMcpServer: (id: string) => call<McpTool[]>("test_mcp_server", { id }),
  listAgents: (reload = false) => call<AgentsView>("list_agents", { reload }),
  listTasks: () => call<Task[]>("list_tasks"),
  taskDetail: (id: string) => call<TaskDetail>("task_detail", { id }),
  startTask: (objective: string, mode: string) => call<Task>("start_task", { objective, mode }),
  cancelTask: (id: string) => call<void>("cancel_task", { id }),
  resumeTask: (id: string) => call<void>("resume_task", { id }),
  deleteTask: (id: string) => call<void>("delete_task", { id }),
  listSchedules: () => call<ScheduleView[]>("list_schedules"),
  addSchedule: (title: string, objective: string, mode: string, cron: string) =>
    call<ScheduleView>("add_schedule", { title, objective, mode, cron }),
  setScheduleEnabled: (id: number, enabled: boolean) => call<void>("set_schedule_enabled", { id, enabled }),
  deleteSchedule: (id: number) => call<void>("delete_schedule", { id }),
  runScheduleNow: (id: number) => call<string>("run_schedule_now", { id }),
  /** Abonnement aux événements des tâches de fond ; renvoie la fonction de désabonnement. */
  onAppEvent: (handler: (e: AppEvent) => void): (() => void) => {
    if (!inTauri) return mockListen(handler);
    const un = listen<AppEvent>("claudio://event", (e) => handler(e.payload));
    return () => {
      un.then((f) => f());
    };
  },
  /** Sélecteur de dossier natif (saisie manuelle en mode simulé). */
  pickFolder: async (): Promise<string | null> => {
    if (!inTauri) return window.prompt("Chemin du dossier", "C:\\Users\\vous\\Documents");
    const r = await openDialog({ directory: true, multiple: false, title: "Choisir un dossier pour Claudio" });
    return typeof r === "string" ? r : null;
  },
};

/** Noms lisibles des outils. */
export function toolLabel(name: string): string {
  const labels: Record<string, string> = {
    list_directory: "Contenu du dossier",
    read_file: "Lecture de fichier",
    search_files: "Recherche de fichiers",
    write_file: "Écriture de fichier",
    create_directory: "Création de dossier",
    move_path: "Déplacement",
    delete_path: "Mise à la corbeille",
    web_fetch: "Page web",
    web_search: "Recherche web",
    run_powershell: "PowerShell",
  };
  if (name.startsWith("mcp__")) {
    const [, server, tool] = name.split("__");
    return `MCP ${server} · ${tool}`;
  }
  return labels[name] ?? name;
}

export const taskStatusLabels: Record<TaskStatus, string> = {
  planning: "Planification",
  running: "En cours",
  done: "Terminée",
  partial: "Partielle",
  failed: "En échec",
  cancelled: "Annulée",
  interrupted: "Interrompue",
};

export const stepStatusLabels: Record<StepStatus, string> = {
  pending: "À faire",
  running: "En cours",
  waiting: "Attend votre accord",
  done: "Terminée",
  failed: "En échec",
  skipped: "Non exécutée",
  cancelled: "Annulée",
};

export const routingLabels: Record<RoutingMode, string> = {
  local: "Local uniquement",
  auto: "Automatique",
  cloud: "Cloud d'abord",
};

export const placementLabels: Record<Placement, string> = {
  gpu: "Carte graphique",
  hybrid: "Hybride GPU + RAM",
  cpu: "Processeur",
  toobig: "Trop gros",
};

export function formatUsd(v: number | null | undefined): string {
  if (v == null) return "—";
  return v.toLocaleString("fr-FR", { style: "currency", currency: "USD", maximumFractionDigits: v < 1 ? 4 : 2 });
}

export const modelKey = (id: string, quant: string) => `${id}:${quant}`;

export function activeLabel(m: ActiveModel | null, installed: InstalledModel[]): string {
  if (!m) return "Aucun modèle";
  if (m.source === "ollama") return `${m.name} (Ollama)`;
  const found = installed.find((i) => i.key === m.id);
  return found ? `${found.name} · ${found.quant}` : m.id;
}

export function backendLabel(b: Backend): string {
  switch (b.kind) {
    case "cuda":
      return `NVIDIA CUDA ${b.version}`;
    case "vulkan":
      return "Vulkan";
    case "cpu":
      return "Processeur";
  }
}
