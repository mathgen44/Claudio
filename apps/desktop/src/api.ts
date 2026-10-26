// Accès aux commandes du cœur Rust. Hors de Tauri (aperçu dans un navigateur),
// une implémentation simulée prend le relais.

import { Channel, invoke } from "@tauri-apps/api/core";
import { mockInvoke } from "./mock";

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
  placement: "gpu" | "cpu";
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
  created_at: string;
  updated_at: string;
}

export type Role = "system" | "user" | "assistant";

export interface Message {
  id: number;
  session_id: string;
  role: Role;
  content: string;
  model: string | null;
  tokens: number | null;
  duration_ms: number | null;
  created_at: string;
}

export type ChatEvent = { type: "token"; text: string } | { type: "reasoning"; text: string };

export interface Reply {
  message: Message;
  reasoning: string;
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
  openFolder: (which: "logs" | "models" | "data") => call<void>("open_folder", { which }),
  sessions: () => call<Session[]>("list_sessions"),
  createSession: () => call<Session>("create_session"),
  renameSession: (id: string, title: string) => call<void>("rename_session", { id, title }),
  deleteSession: (id: string) => call<void>("delete_session", { id }),
  messages: (sessionId: string) => call<Message[]>("list_messages", { sessionId }),
  sendMessage: (sessionId: string, text: string, onEvent: (e: ChatEvent) => void) =>
    call<Reply>("send_message", { sessionId, text, onEvent: channel(onEvent) }),
  cancelChat: (sessionId: string) => call<void>("cancel_chat", { sessionId }),
};

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
