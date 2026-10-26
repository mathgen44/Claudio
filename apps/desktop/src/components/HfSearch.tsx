// Recherche de modèles GGUF sur Hugging Face et téléchargement d'une variante.

import { ArrowLeft, Download, ExternalLink, Heart, Lock, Search } from "lucide-react";
import { useState } from "react";
import { api, modelKey, placementLabels, type HfRepo, type HfSummary, type HfVariant } from "../api";
import { useStore } from "../store";
import { Badge, Button, Card, ErrorNote, Spinner, formatBytes } from "../ui";
import { DownloadRow } from "./DownloadRow";

const compact = (n: number) => n.toLocaleString("fr-FR", { notation: "compact" });

export function HfSearch() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<HfSummary[] | null>(null);
  const [repo, setRepo] = useState<HfRepo | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async <T,>(job: () => Promise<T>, done: (v: T) => void) => {
    setLoading(true);
    setError(null);
    try {
      done(await job());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const search = () => query.trim() && run(() => api.hfSearch(query), (r) => (setResults(r), setRepo(null)));
  const open = (id: string) => run(() => api.hfRepo(id), setRepo);

  return (
    <div className="space-y-3">
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          // Un identifiant complet (« auteur/dépôt ») ouvre directement le dépôt.
          if (/^[\w.-]+\/[\w.-]+$/.test(query.trim())) open(query.trim());
          else search();
        }}
      >
        <div className="flex flex-1 items-center gap-2 rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 focus-within:border-[var(--accent)]">
          <Search size={16} className="text-[var(--muted)]" />
          <input
            className="flex-1 bg-transparent py-2 outline-none"
            placeholder="Nom de modèle (ex. qwen, mistral) ou dépôt complet (auteur/dépôt)"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <Button variant="primary" type="submit" disabled={!query.trim() || loading}>
          {loading ? <Spinner /> : "Rechercher"}
        </Button>
      </form>
      <ErrorNote message={error} />

      {repo ? (
        <RepoDetails repo={repo} onBack={() => setRepo(null)} onRefresh={() => open(repo.id)} hasResults={!!results} />
      ) : (
        results && (
          <Card className="divide-y divide-[var(--border)] p-0">
            {results.length === 0 && <p className="p-4 text-[var(--muted)]">Aucun modèle GGUF trouvé.</p>}
            {results.map((r) => (
              <button
                key={r.id}
                className="flex w-full items-center justify-between gap-3 px-4 py-2.5 text-left hover:bg-[var(--panel-2)]"
                onClick={() => open(r.id)}
              >
                <span className="min-w-0 truncate font-medium">{r.id}</span>
                <span className="flex shrink-0 items-center gap-3 text-xs text-[var(--muted)]">
                  {r.license && <Badge>{r.license}</Badge>}
                  <span>{compact(r.downloads)} téléchargements</span>
                  <span className="flex items-center gap-1">
                    <Heart size={12} /> {compact(r.likes)}
                  </span>
                </span>
              </button>
            ))}
          </Card>
        )
      )}
    </div>
  );
}

function RepoDetails({
  repo,
  onBack,
  onRefresh,
  hasResults,
}: {
  repo: HfRepo;
  onBack: () => void;
  onRefresh: () => void;
  hasResults: boolean;
}) {
  const { status, downloads, downloadHf } = useStore();
  const [accepted, setAccepted] = useState(!repo.license_needs_review);
  const [showAll, setShowAll] = useState(false);
  const usable = repo.variants.filter((v) => v.fit.placement !== "toobig");
  const visible = showAll ? repo.variants : usable;

  const download = async (v: HfVariant) => {
    if (await downloadHf(repo.id, v.quant)) onRefresh();
  };

  return (
    <Card className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          {hasResults && (
            <button className="mb-1 flex items-center gap-1 text-sm text-[var(--muted)] hover:text-[var(--text)]" onClick={onBack}>
              <ArrowLeft size={14} /> Résultats
            </button>
          )}
          <h3 className="truncate text-lg font-semibold">{repo.id}</h3>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-sm text-[var(--muted)]">
            <Badge tone={repo.license_needs_review ? "neutral" : "ok"}>licence {repo.license ?? "non indiquée"}</Badge>
            {repo.gated && (
              <Badge>
                <Lock size={11} className="mr-1" /> accès restreint
              </Badge>
            )}
            <span>{compact(repo.downloads)} téléchargements</span>
          </div>
        </div>
        <a href={repo.url} target="_blank" rel="noreferrer" className="flex items-center gap-1 text-sm text-[var(--accent)]">
          Voir la page <ExternalLink size={13} />
        </a>
      </div>

      {repo.gated && !status?.has_hf_token && (
        <p className="text-sm text-[var(--muted)]">
          Ce dépôt est restreint : acceptez ses conditions sur sa page Hugging Face puis enregistrez votre jeton dans les
          Paramètres.
        </p>
      )}
      {repo.license_needs_review && (
        <label className="flex items-start gap-2 rounded-lg bg-[var(--panel-2)] p-3 text-sm">
          <input type="checkbox" className="mt-0.5 accent-[var(--accent)]" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
          <span>
            La licence « {repo.license ?? "non indiquée"} » n'est pas une licence libre courante. J'ai lu ses conditions sur la page
            du modèle et je les accepte.
          </span>
        </label>
      )}

      {repo.variants.length === 0 && <p className="text-[var(--muted)]">Aucun fichier GGUF dans ce dépôt.</p>}
      {repo.variants.length > 0 && usable.length === 0 && !showAll && (
        <p className="text-[var(--muted)]">Aucune variante ne tient en mémoire sur cette machine.</p>
      )}
      <div className="divide-y divide-[var(--border)] rounded-lg border border-[var(--border)]">
        {visible.map((v) => {
          const key = modelKey(repo.id, v.quant);
          const dl = downloads[key];
          return (
            <div key={v.quant} className="space-y-2 px-3 py-2.5">
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="font-medium">{v.quant}</span>
                  <span className="text-sm text-[var(--muted)]">{formatBytes(v.size)}</span>
                  {v.files.length > 1 && <Badge>{v.files.length} fichiers</Badge>}
                  <Badge tone={v.fit.placement === "gpu" ? "ok" : "neutral"}>{placementLabels[v.fit.placement]}</Badge>
                  <span className="text-xs text-[var(--muted)]">
                    ~{v.fit.params_b.toFixed(1).replace(".", ",")} G paramètres · ~{formatBytes(v.fit.memory_needed)} de mémoire
                  </span>
                </div>
                {v.installed ? (
                  <span className="text-sm text-[var(--ok)]">Installé</span>
                ) : (
                  !dl && (
                    <Button disabled={!accepted || v.fit.placement === "toobig"} onClick={() => download(v)}>
                      <Download size={16} /> Télécharger
                    </Button>
                  )
                )}
              </div>
              {dl && <DownloadRow id={key} state={dl} />}
            </div>
          );
        })}
      </div>
      {usable.length < repo.variants.length && (
        <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
          <input type="checkbox" className="accent-[var(--accent)]" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} />
          Afficher aussi les {repo.variants.length - usable.length} variantes trop grosses pour cette machine
        </label>
      )}
    </Card>
  );
}
