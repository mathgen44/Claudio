// « Mise en route » : machine détectée, moteur, premier modèle.

import { Check, Download, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { api, backendLabel, modelKey, type Recommendation } from "../api";
import { DownloadRow } from "../components/DownloadRow";
import { MachineCard } from "../components/MachineCard";
import { RUNTIME_KEY, useStore } from "../store";
import { Badge, Button, Card, ErrorNote, Spinner, formatBytes } from "../ui";

export function Onboarding({ onDone }: { onDone: () => void }) {
  const { status, downloads, installRuntime, downloadModel } = useStore();
  const [recos, setRecos] = useState<Recommendation[] | null>(null);
  const [choice, setChoice] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.recommendations().then(setRecos, (e) => setError(String(e)));
  }, [status?.installed_models.length]);

  if (!status) return null;
  const runtimeDl = downloads[RUNTIME_KEY];
  const hasModel = status.installed_models.length > 0 || status.active_model?.source === "ollama";
  // Une seule quantification par modèle, la meilleure, pour une liste lisible.
  const options = (recos ?? []).filter((r, i, all) => all.findIndex((x) => x.model_id === r.model_id) === i).slice(0, 4);
  const picked = options[choice];
  const pickedDl = picked && downloads[modelKey(picked.model_id, picked.quant)];

  const finish = async () => {
    await api.updateSettings({ onboardingDone: true });
    onDone();
  };

  return (
    <div className="mx-auto max-w-3xl space-y-6 px-6 py-10">
      <div>
        <div className="flex items-center gap-2 text-[var(--accent)]">
          <Sparkles size={20} />
          <span className="font-medium">Mise en route</span>
        </div>
        <h1 className="mt-2 text-2xl font-semibold">Bienvenue dans Claudio</h1>
        <p className="mt-1 text-[var(--muted)]">
          Trois étapes pour discuter avec un assistant qui tourne entièrement sur ton ordinateur.
        </p>
      </div>

      <Step n={1} title="Ta machine" done>
        <MachineCard status={status} />
      </Step>

      <Step n={2} title="Moteur d'inférence" done={!!status.runtime}>
        <Card>
          {status.runtime ? (
            <p>
              llama.cpp {status.runtime.tag} installé · accélération <b>{backendLabel(status.runtime.backend)}</b>
            </p>
          ) : runtimeDl ? (
            <DownloadRow id={RUNTIME_KEY} state={runtimeDl} />
          ) : (
            <div className="flex items-center justify-between gap-4">
              <p className="text-[var(--muted)]">
                Claudio télécharge la version officielle de llama.cpp la mieux adaptée à ta carte graphique.
              </p>
              <Button variant="primary" onClick={installRuntime}>
                <Download size={16} /> Installer
              </Button>
            </div>
          )}
        </Card>
      </Step>

      <Step n={3} title="Premier modèle" done={hasModel}>
        <Card className="space-y-3">
          {recos == null && !error && (
            <p className="flex items-center gap-2 text-[var(--muted)]">
              <Spinner /> Calcul des modèles adaptés…
            </p>
          )}
          <ErrorNote message={error} />
          {recos && options.length === 0 && (
            <p className="text-[var(--muted)]">Aucun modèle du catalogue ne tient dans la mémoire de cette machine.</p>
          )}
          {options.map((r, i) => (
            <label
              key={r.model_id}
              className={`flex cursor-pointer gap-3 rounded-lg border p-3 ${
                i === choice ? "border-[var(--accent)] bg-[var(--accent-soft)]/40" : "border-[var(--border)]"
              }`}
            >
              <input type="radio" className="mt-1 accent-[var(--accent)]" checked={i === choice} onChange={() => setChoice(i)} />
              <div className="flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{r.model_name}</span>
                  <Badge>{r.quant}</Badge>
                  {i === 0 && <Badge tone="accent">Recommandé</Badge>}
                  {r.installed && <Badge tone="ok">Installé</Badge>}
                </div>
                <p className="mt-0.5 text-[var(--muted)]">{r.description}</p>
                <p className="mt-1 text-xs text-[var(--muted)]">
                  {formatBytes(r.file_size)} à télécharger · {r.placement === "gpu" ? "sur la carte graphique" : "sur le processeur"} ·
                  licence {r.license}
                </p>
              </div>
            </label>
          ))}
          {picked && (
            <div className="pt-1">
              {pickedDl ? (
                <DownloadRow id={modelKey(picked.model_id, picked.quant)} state={pickedDl} />
              ) : (
                !picked.installed && (
                  <Button
                    variant="primary"
                    onClick={() => downloadModel(picked.model_id, picked.quant, `${picked.model_name} · ${picked.quant}`)}
                  >
                    <Download size={16} /> Télécharger {picked.model_name} ({formatBytes(picked.file_size)})
                  </Button>
                )
              )}
            </div>
          )}
          {status.ollama_models && status.ollama_models.length > 0 && (
            <p className="text-sm text-[var(--muted)]">
              Ollama est détecté avec {status.ollama_models.length} modèle(s) : tu pourras aussi les utiliser depuis l'écran
              Modèles.
            </p>
          )}
        </Card>
      </Step>

      <div className="flex justify-end gap-3">
        <Button variant="ghost" onClick={finish}>
          Passer
        </Button>
        <Button variant="primary" disabled={!status.runtime || !hasModel} onClick={finish}>
          Commencer à discuter
        </Button>
      </div>
    </div>
  );
}

function Step({ n, title, done, children }: { n: number; title: string; done?: boolean; children: React.ReactNode }) {
  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 font-medium">
        <span
          className={`flex size-6 items-center justify-center rounded-full text-xs ${
            done ? "bg-[var(--accent)] text-[var(--accent-text)]" : "bg-[var(--panel-2)] text-[var(--muted)]"
          }`}
        >
          {done ? <Check size={14} /> : n}
        </span>
        {title}
      </h2>
      {children}
    </section>
  );
}
