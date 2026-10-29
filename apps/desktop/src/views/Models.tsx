// Modèles : actif, installés, catalogue adapté à la machine, Ollama.

import { Check, Download, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, modelKey, placementLabels, type ActiveModel, type Recommendation } from "../api";
import { DownloadRow } from "../components/DownloadRow";
import { HfSearch } from "../components/HfSearch";
import { useStore } from "../store";
import { Badge, Button, Card, ErrorNote, PageHeader, Spinner, formatBytes } from "../ui";

const sameModel = (a: ActiveModel | null, b: ActiveModel) =>
  !!a && a.source === b.source && (a.source === "local" ? a.id === (b as typeof a).id : a.name === (b as typeof a).name);

export function Models() {
  const { status, refresh, downloads, downloadModel } = useStore();
  const [recos, setRecos] = useState<Recommendation[] | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.recommendations().then(setRecos, (e) => setError(String(e)));
  }, [status?.installed_models.length]);

  if (!status) return null;

  const activate = async (m: ActiveModel) => {
    setError(null);
    try {
      await api.setActiveModel(m);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (key: string, name: string) => {
    if (!window.confirm(`Supprimer ${name} du disque ? Il faudra le retélécharger pour l'utiliser.`)) return;
    try {
      await api.removeModel(key);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const best = new Set<string>();
  const visible = (recos ?? []).filter((r) => {
    if (showAll) return true;
    if (best.has(r.model_id)) return false;
    best.add(r.model_id);
    return true;
  });

  return (
    <div className="mx-auto max-w-4xl space-y-8 px-6 py-8">
      <PageHeader
        title="Modèles"
        subtitle={`Stockés dans ${status.models_dir}`}
        actions={
          <Button variant="ghost" onClick={() => api.openFolder("models")}>
            Ouvrir le dossier
          </Button>
        }
      />
      <ErrorNote message={error} />

      <section className="space-y-3">
        <h2 className="font-medium">Installés</h2>
        {status.installed_models.length === 0 && (
          <p className="text-[var(--muted)]">Aucun modèle téléchargé pour l'instant.</p>
        )}
        {status.installed_models.map((m) => {
          const ref: ActiveModel = { source: "local", id: m.key };
          const isActive = sameModel(status.active_model, ref);
          const isEmbedding = status.embedding_models.includes(m.key);
          return (
            <Card key={m.key} className="flex items-center justify-between gap-4 py-3">
              <div>
                <div className="flex items-center gap-2">
                  <span className="font-medium">{m.name}</span>
                  <Badge>{m.quant}</Badge>
                  {isActive && <Badge tone="accent">Actif</Badge>}
                  {status.loaded_models.includes(m.key) && <Badge tone="ok">En mémoire</Badge>}
                </div>
                <p className="text-xs text-[var(--muted)]">
                  {formatBytes(m.size)} · {m.repo}
                </p>
              </div>
              <div className="flex gap-1">
                {isEmbedding ? (
                  <Badge>embeddings (Mémoire)</Badge>
                ) : (
                  !isActive && <Button onClick={() => activate(ref)}>Utiliser</Button>
                )}
                <Button variant="danger" onClick={() => remove(m.key, m.name)} aria-label="Supprimer">
                  <Trash2 size={16} />
                </Button>
              </div>
            </Card>
          );
        })}
      </section>

      {status.ollama_models && status.ollama_models.length > 0 && (
        <section className="space-y-3">
          <h2 className="font-medium">Ollama</h2>
          {status.ollama_models.map((m) => {
            const ref: ActiveModel = { source: "ollama", name: m.name };
            const isActive = sameModel(status.active_model, ref);
            return (
              <Card key={m.name} className="flex items-center justify-between gap-4 py-3">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{m.name}</span>
                  {m.details.parameter_size && <Badge>{m.details.parameter_size}</Badge>}
                  {m.details.quantization_level && <Badge>{m.details.quantization_level}</Badge>}
                  {isActive && <Badge tone="accent">Actif</Badge>}
                </div>
                {!isActive && <Button onClick={() => activate(ref)}>Utiliser</Button>}
              </Card>
            );
          })}
        </section>
      )}

      <section className="space-y-3">
        <div className="flex items-center justify-between">
          <div>
            <h2 className="font-medium">Catalogue adapté à cette machine</h2>
            <p className="text-sm text-[var(--muted)]">
              Classés selon la qualité et ce qui tient dans {formatBytes(status.profile.vram_budget || status.profile.ram_budget)} de
              mémoire disponible.
            </p>
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input type="checkbox" className="accent-[var(--accent)]" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} />
            Toutes les quantifications
          </label>
        </div>
        {recos == null && !error && <Spinner className="text-[var(--muted)]" />}
        {visible.map((r, i) => {
          const key = modelKey(r.model_id, r.quant);
          const dl = downloads[key];
          return (
            <Card key={key} className="space-y-2 py-3">
              <div className="flex items-center justify-between gap-4">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{r.model_name}</span>
                    <Badge>{r.quant}</Badge>
                    {i === 0 && !showAll && <Badge tone="accent">Recommandé</Badge>}
                    <Badge>{placementLabels[r.placement]}</Badge>
                  </div>
                  <p className="mt-0.5 text-sm text-[var(--muted)]">{r.description}</p>
                  <p className="text-xs text-[var(--muted)]">
                    {formatBytes(r.file_size)} · ~{formatBytes(r.memory_needed)} de mémoire avec {r.context} jetons de contexte ·
                    licence {r.license}
                  </p>
                </div>
                {r.installed ? (
                  <span className="flex items-center gap-1 text-sm text-[var(--ok)]">
                    <Check size={16} /> Installé
                  </span>
                ) : (
                  !dl && (
                    <Button onClick={() => downloadModel(r.model_id, r.quant, `${r.model_name} · ${r.quant}`)}>
                      <Download size={16} /> Télécharger
                    </Button>
                  )
                )}
              </div>
              {dl && <DownloadRow id={key} state={dl} />}
            </Card>
          );
        })}
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="font-medium">Rechercher sur Hugging Face</h2>
          <p className="text-sm text-[var(--muted)]">
            Tous les modèles au format GGUF, avec une estimation de ce qui tient sur cette machine.
          </p>
        </div>
        <HfSearch />
      </section>
    </div>
  );
}
