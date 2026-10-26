// Paramètres : conversation, moteur, Hugging Face, dossiers.

import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { api, backendLabel, type Config, type SettingsPatch } from "../api";
import { autostartEnabled, findUpdate, installUpdate, setAutostart } from "../updates";
import { DownloadRow } from "../components/DownloadRow";
import { MachineCard } from "../components/MachineCard";
import { RUNTIME_KEY, useStore } from "../store";
import { Button, Card, ErrorNote, PageHeader } from "../ui";

const CONTEXTS = [4096, 8192, 16384, 32768];

export function Settings({ onRestartOnboarding }: { onRestartOnboarding: () => void }) {
  const { status, refresh, downloads, installRuntime } = useStore();
  const [cfg, setCfg] = useState<Config | null>(null);
  const [prompt, setPrompt] = useState("");
  const [token, setToken] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [autostart, setAutostartState] = useState(false);

  useEffect(() => {
    autostartEnabled().then(setAutostartState).catch(() => {});
  }, []);

  useEffect(() => {
    api.config().then((c) => {
      setCfg(c);
      setPrompt(c.system_prompt);
    });
  }, []);

  if (!cfg || !status) return null;

  const update = async (patch: SettingsPatch, message = "Enregistré") => {
    setError(null);
    try {
      setCfg(await api.updateSettings(patch));
      setSaved(message);
      setTimeout(() => setSaved(null), 2000);
      refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const saveToken = async (value: string | null) => {
    try {
      await api.setHfToken(value);
      setToken("");
      await refresh();
      setSaved(value ? "Jeton enregistré dans le Gestionnaire d'identifiants Windows" : "Jeton supprimé");
    } catch (e) {
      setError(String(e));
    }
  };

  const checkNow = async () => {
    setError(null);
    setSaved("Recherche d'une mise à jour…");
    try {
      const u = await findUpdate();
      if (!u) {
        setSaved("Claudio est à jour");
        return;
      }
      if (!window.confirm(`Claudio ${u.version} est disponible. L'installer maintenant ? Claudio redémarrera.`)) return;
      setSaved(`Téléchargement de Claudio ${u.version}…`);
      await installUpdate(u, () => {});
    } catch (e) {
      setSaved(null);
      setError(`Mise à jour impossible : ${e}`);
    }
  };

  const runtimeDl = downloads[RUNTIME_KEY];
  const select = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";

  return (
    <div className="mx-auto max-w-3xl space-y-6 px-6 py-8">
      <PageHeader title="Paramètres" subtitle={saved ?? undefined} />
      <ErrorNote message={error} />

      <Card className="space-y-4">
        <h2 className="font-medium">Conversation</h2>
        <Field label="Taille du contexte" hint="Mémoire de la conversation, en jetons. Plus grand = plus de mémoire vidéo.">
          <select
            className={select}
            value={cfg.context_size ?? ""}
            onChange={(e) => update({ contextSize: e.target.value ? Number(e.target.value) : null })}
          >
            <option value="">Automatique (8 192)</option>
            {CONTEXTS.map((c) => (
              <option key={c} value={c}>
                {c.toLocaleString("fr-FR")}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Réflexion avant de répondre" hint="Plus lent mais plus juste. En automatique, réservée aux modèles de 9 milliards de paramètres et plus.">
          <select
            className={select}
            value={cfg.thinking == null ? "" : String(cfg.thinking)}
            onChange={(e) => update({ thinking: e.target.value === "" ? null : e.target.value === "true" })}
          >
            <option value="">Automatique</option>
            <option value="true">Activée</option>
            <option value="false">Désactivée</option>
          </select>
        </Field>
        <Field label="Instructions de l'assistant" hint="Prompt système envoyé au début de chaque conversation.">
          <textarea
            className="min-h-24 w-full rounded-lg border border-[var(--border)] bg-[var(--panel)] p-3"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
          />
          <Button className="mt-2" disabled={prompt === cfg.system_prompt} onClick={() => update({ systemPrompt: prompt })}>
            Enregistrer
          </Button>
        </Field>
      </Card>

      <Card className="space-y-4">
        <h2 className="font-medium">Moteur local</h2>
        <p className="text-[var(--muted)]">
          {status.runtime
            ? `llama.cpp ${status.runtime.tag} · ${backendLabel(status.runtime.backend)}`
            : "llama.cpp n'est pas installé."}
        </p>
        <Field label="Accélération" hint="Automatique choisit CUDA pour NVIDIA, Vulkan pour les autres cartes, sinon le processeur.">
          <div className="flex gap-2">
            <select
              className={select}
              value={cfg.llama_backend ?? ""}
              onChange={(e) => update({ llamaBackend: e.target.value || null }, "Réinstalle le moteur pour appliquer ce choix")}
            >
              <option value="">Automatique</option>
              <option value="cuda">CUDA (NVIDIA)</option>
              <option value="vulkan">Vulkan</option>
              <option value="cpu">Processeur seul</option>
            </select>
            <Button onClick={installRuntime} disabled={!!runtimeDl}>
              <RefreshCw size={16} /> {status.runtime ? "Mettre à jour" : "Installer"}
            </Button>
          </div>
        </Field>
        {runtimeDl && <DownloadRow id={RUNTIME_KEY} state={runtimeDl} />}
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            className="accent-[var(--accent)]"
            checked={cfg.use_ollama}
            onChange={(e) => update({ useOllama: e.target.checked })}
          />
          Proposer les modèles d'Ollama s'il est installé
        </label>
      </Card>

      <Card className="space-y-4">
        <h2 className="font-medium">Hugging Face</h2>
        <Field
          label="Jeton d'accès"
          hint="Facultatif : nécessaire seulement pour les modèles à accès restreint. Stocké chiffré dans le Gestionnaire d'identifiants Windows."
        >
          <div className="flex gap-2">
            <input
              type="password"
              className="flex-1 rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2"
              placeholder={status.has_hf_token ? "Un jeton est enregistré" : "hf_…"}
              value={token}
              onChange={(e) => setToken(e.target.value)}
              autoComplete="off"
            />
            <Button disabled={!token.trim()} onClick={() => saveToken(token)}>
              Enregistrer
            </Button>
            {status.has_hf_token && (
              <Button variant="danger" onClick={() => saveToken(null)}>
                Supprimer
              </Button>
            )}
          </div>
        </Field>
      </Card>

      <Card className="space-y-4">
        <div className="flex items-center justify-between">
          <h2 className="font-medium">Machine</h2>
          <Button
            variant="ghost"
            onClick={async () => {
              await api.refreshHardware();
              await refresh();
            }}
          >
            <RefreshCw size={16} /> Analyser à nouveau
          </Button>
        </div>
        <MachineCard status={status} />
      </Card>

      <Card className="space-y-3">
        <h2 className="font-medium">Démarrage</h2>
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            className="mt-1 accent-[var(--accent)]"
            checked={autostart}
            onChange={async (e) => {
              const on = e.target.checked;
              try {
                await setAutostart(on);
                setAutostartState(on);
                setSaved(on ? "Claudio démarrera avec Windows" : "Démarrage automatique désactivé");
              } catch (err) {
                setError(String(err));
              }
            }}
          />
          <span>
            Lancer Claudio à l'ouverture de session Windows
            <span className="block text-sm text-[var(--muted)]">
              Il démarre discrètement dans la zone de notification : tâches planifiées, Telegram et mot d'éveil restent disponibles.
            </span>
          </span>
        </label>
      </Card>

      <Card className="space-y-3">
        <h2 className="font-medium">Mises à jour</h2>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            className="accent-[var(--accent)]"
            checked={cfg.check_updates}
            onChange={(e) => update({ checkUpdates: e.target.checked })}
          />
          Chercher une nouvelle version au démarrage
        </label>
        <p className="text-sm text-[var(--muted)]">
          Les mises à jour sont signées : Claudio n'installe que les versions dont la signature correspond à sa clé publique.
        </p>
        <Button onClick={checkNow}>
          <RefreshCw size={16} /> Rechercher maintenant
        </Button>
      </Card>

      <Card className="flex flex-wrap gap-2">
        <Button onClick={() => api.openFolder("logs")}>Ouvrir les journaux</Button>
        <Button onClick={() => api.openFolder("data")}>Ouvrir les données</Button>
        <Button variant="ghost" onClick={onRestartOnboarding}>
          Revoir la mise en route
        </Button>
      </Card>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <div>
        <div className="font-medium">{label}</div>
        {hint && <div className="text-sm text-[var(--muted)]">{hint}</div>}
      </div>
      {children}
    </div>
  );
}
