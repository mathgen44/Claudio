// Implémentation simulée des commandes, pour développer l'interface dans un
// navigateur sans le cœur Rust (`npm run dev`). Jamais utilisée dans l'application.

import type {
  ActiveModel,
  ChatEvent,
  Config,
  InstallEvent,
  InstalledModel,
  Message,
  Progress,
  Recommendation,
  Session,
  Status,
} from "./api";

const GB = 1024 ** 3;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const config: Config = {
  models_dir: null,
  active_model: null,
  context_size: null,
  use_ollama: true,
  llama_backend: null,
  thinking: null,
  system_prompt: "Tu es Claudio, un assistant personnel qui fonctionne sur l'ordinateur de l'utilisateur.",
  onboarding_done: new URLSearchParams(location.search).has("ready"),
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

const installed: InstalledModel[] = [];
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
      const s = { id: crypto.randomUUID(), title: "Nouvelle conversation", created_at: now, updated_at: now };
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
      const on = args.onEvent as (e: ChatEvent) => void;
      const s = sessions.find((x) => x.id === args.sessionId)!;
      const now = () => new Date().toISOString();
      messages.push({ id: nextId++, session_id: s.id, role: "user", content: args.text, model: null, tokens: null, duration_ms: null, created_at: now() });
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
      const msg: Message = { id: nextId++, session_id: s.id, role: "assistant", content: answer, model: "gemma-4-12b:Q6_K", tokens: 64, duration_ms: 900, created_at: now() };
      messages.push(msg);
      s.updated_at = now();
      return r({ message: msg, reasoning: "", tokens_per_sec: 88.4, cancelled: false });
    }
    default:
      throw `commande simulée inconnue : ${cmd}`;
  }
}
