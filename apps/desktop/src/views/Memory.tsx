// Mémoire : souvenirs (modifiables), documents indexés, compétences apprises.

import clsx from "clsx";
import {
  Check,
  Download,
  FileSearch,
  FolderOpen,
  FolderPlus,
  Lightbulb,
  Pencil,
  Pin,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  api,
  MEMORY_CATEGORIES,
  type Config,
  type DocHit,
  type KnowledgeStatus,
  type Memory as MemoryItem,
  type MemorySettings,
  type Skill,
  type SkillProposal,
} from "../api";
import { DownloadRow } from "../components/DownloadRow";
import { useStore } from "../store";
import { Badge, Button, Card, ErrorNote, PageHeader, Spinner, formatBytes } from "../ui";

type Tab = "memories" | "documents" | "skills";
const input = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";
const SOURCE_LABEL: Record<string, string> = { manuel: "ajouté par vous", assistant: "noté par l'assistant", auto: "retenu automatiquement" };

export function Memory() {
  const [tab, setTab] = useState<Tab>("memories");
  const [cfg, setCfg] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.config().then(setCfg, (e) => setError(String(e)));
  }, []);

  const setSettings = async (patch: Partial<MemorySettings>) => {
    if (!cfg) return;
    const memory = { ...cfg.memory, ...patch };
    try {
      await api.setMemorySettings(memory);
      setCfg({ ...cfg, memory });
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="mx-auto max-w-5xl space-y-5 px-6 py-8">
      <PageHeader title="Mémoire" subtitle="Ce que Claudio sait de vous, les documents où il peut chercher, et les savoir-faire qu'il a appris." />
      <div className="flex gap-1 border-b border-[var(--border)]">
        {(
          [
            ["memories", "Souvenirs"],
            ["documents", "Documents"],
            ["skills", "Compétences"],
          ] as [Tab, string][]
        ).map(([t, label]) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={clsx(
              "-mb-px border-b-2 px-4 py-2 text-sm font-medium",
              tab === t ? "border-[var(--accent)] text-[var(--accent)]" : "border-transparent text-[var(--muted)] hover:text-[var(--text)]",
            )}
          >
            {label}
          </button>
        ))}
      </div>
      <ErrorNote message={error} />
      {tab === "memories" && cfg && <Memories settings={cfg.memory} onSettings={setSettings} onError={setError} />}
      {tab === "documents" && <Documents onError={setError} />}
      {tab === "skills" && cfg && <Skills settings={cfg.memory} onSettings={setSettings} onError={setError} />}
    </div>
  );
}

function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint: string }) {
  return (
    <label className="flex items-start gap-3">
      <input type="checkbox" className="mt-1 accent-[var(--accent)]" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="font-medium">{label}</span>
        <span className="block text-sm text-[var(--muted)]">{hint}</span>
      </span>
    </label>
  );
}

function Memories({
  settings,
  onSettings,
  onError,
}: {
  settings: MemorySettings;
  onSettings: (p: Partial<MemorySettings>) => void;
  onError: (e: string | null) => void;
}) {
  const [list, setList] = useState<MemoryItem[]>([]);
  const [filter, setFilter] = useState("");
  const [content, setContent] = useState("");
  const [category, setCategory] = useState("préférence");
  const [editing, setEditing] = useState<MemoryItem | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await api.listMemories(filter || null));
    } catch (e) {
      onError(String(e));
    }
  }, [filter, onError]);

  useEffect(() => {
    load();
    return api.onAppEvent((e) => e.type === "memory_learned" && load());
  }, [load]);

  const act = async (job: () => Promise<unknown>) => {
    onError(null);
    try {
      await job();
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="space-y-4">
      <Card className="space-y-3">
        <Toggle
          checked={settings.recall}
          onChange={(recall) => onSettings({ recall })}
          label="Utiliser les souvenirs dans les réponses"
          hint="Les souvenirs utiles à la question (et ceux épinglés) sont donnés à l'assistant."
        />
        <Toggle
          checked={settings.auto_extract}
          onChange={(auto_extract) => onSettings({ auto_extract })}
          label="Retenir automatiquement"
          hint="Après chaque échange, le modèle local note les faits durables que vous avez partagés. Rien n'est envoyé au cloud pour cela."
        />
      </Card>

      <Card className="flex flex-wrap gap-2">
        <input
          className={`${input} min-w-64 flex-1`}
          placeholder="Ajouter un souvenir, ex. « Je préfère des réponses courtes »"
          value={content}
          onChange={(e) => setContent(e.target.value)}
        />
        <select className={input} value={category} onChange={(e) => setCategory(e.target.value)}>
          {MEMORY_CATEGORIES.map((c) => (
            <option key={c}>{c}</option>
          ))}
        </select>
        <Button
          variant="primary"
          disabled={!content.trim()}
          onClick={() => act(async () => (await api.addMemory(content, category), setContent("")))}
        >
          <Plus size={16} /> Ajouter
        </Button>
      </Card>

      <div className="flex items-center gap-2">
        <div className="flex flex-1 items-center gap-2 rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3">
          <Search size={15} className="text-[var(--muted)]" />
          <input className="flex-1 bg-transparent py-2 outline-none" placeholder="Filtrer" value={filter} onChange={(e) => setFilter(e.target.value)} />
        </div>
        {list.length > 0 && (
          <Button
            variant="danger"
            onClick={() => window.confirm("Effacer tous les souvenirs ? Cette action est définitive.") && act(api.deleteAllMemories)}
          >
            Tout effacer
          </Button>
        )}
      </div>

      {list.length === 0 && <p className="text-sm text-[var(--muted)]">Aucun souvenir pour l'instant.</p>}
      <div className="space-y-2">
        {list.map((m) =>
          editing?.id === m.id ? (
            <Card key={m.id} className="space-y-2 py-3">
              <textarea className={`${input} w-full`} value={editing.content} onChange={(e) => setEditing({ ...editing, content: e.target.value })} />
              <div className="flex justify-between gap-2">
                <select className={input} value={editing.category} onChange={(e) => setEditing({ ...editing, category: e.target.value })}>
                  {MEMORY_CATEGORIES.map((c) => (
                    <option key={c}>{c}</option>
                  ))}
                </select>
                <div className="flex gap-2">
                  <Button variant="ghost" onClick={() => setEditing(null)}>
                    Annuler
                  </Button>
                  <Button
                    variant="primary"
                    onClick={() =>
                      act(async () => {
                        await api.updateMemory(editing.id, editing.content, editing.category, editing.pinned);
                        setEditing(null);
                      })
                    }
                  >
                    Enregistrer
                  </Button>
                </div>
              </div>
            </Card>
          ) : (
            <Card key={m.id} className="flex items-start justify-between gap-3 py-3">
              <div className="min-w-0">
                <p>{m.content}</p>
                <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-[var(--muted)]">
                  <Badge>{m.category}</Badge>
                  <span>{SOURCE_LABEL[m.source] ?? m.source}</span>
                  <span>· {new Date(m.updated_at).toLocaleDateString("fr-FR")}</span>
                </div>
              </div>
              <div className="flex shrink-0 gap-1">
                <Button
                  variant="ghost"
                  aria-label={m.pinned ? "Désépingler" : "Épingler (toujours rappelé)"}
                  title={m.pinned ? "Désépingler" : "Épingler : toujours rappelé"}
                  onClick={() => act(() => api.updateMemory(m.id, m.content, m.category, !m.pinned))}
                >
                  <Pin size={15} className={m.pinned ? "fill-current text-[var(--accent)]" : ""} />
                </Button>
                <Button variant="ghost" aria-label="Modifier" onClick={() => setEditing(m)}>
                  <Pencil size={15} />
                </Button>
                <Button variant="danger" aria-label="Supprimer" onClick={() => act(() => api.deleteMemory(m.id))}>
                  <Trash2 size={15} />
                </Button>
              </div>
            </Card>
          ),
        )}
      </div>
    </div>
  );
}

const STATUS_LABEL: Record<string, string> = {
  idle: "À jour",
  scanning: "Lecture des fichiers…",
  embedding: "Calcul des vecteurs…",
  error: "Erreur",
};

function Documents({ onError }: { onError: (e: string | null) => void }) {
  const { downloads, downloadModel } = useStore();
  const [status, setStatus] = useState<KnowledgeStatus | null>(null);
  const [progress, setProgress] = useState<Record<number, { done: number; total: number; path: string }>>({});
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<DocHit[] | null>(null);
  const [searching, setSearching] = useState(false);

  const load = useCallback(async () => {
    try {
      setStatus(await api.knowledgeStatus());
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  useEffect(() => {
    load();
    return api.onAppEvent((e) => {
      if (e.type === "index_progress") setProgress((p) => ({ ...p, [e.source_id]: e }));
      if (e.type === "index_updated") load();
    });
  }, [load]);

  const act = async (job: () => Promise<unknown>) => {
    onError(null);
    try {
      await job();
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  const add = async () => {
    const path = await api.pickFolder();
    if (path) act(() => api.addIndexSource(path));
  };

  const installEmbeddings = async () => {
    if (!status?.recommended) return;
    const [id, quant] = status.recommended;
    if (await downloadModel(id, quant, "Modèle d'embeddings")) await act(api.reindexAll);
  };

  const search = async () => {
    if (!query.trim()) return;
    setSearching(true);
    try {
      setHits(await api.searchDocuments(query));
    } catch (e) {
      onError(String(e));
    } finally {
      setSearching(false);
    }
  };

  if (!status) return <Spinner />;
  const dlKey = status.recommended ? `${status.recommended[0]}:${status.recommended[1]}` : "";

  return (
    <div className="space-y-4">
      <Card className="space-y-2">
        {status.embedding_model ? (
          <p className="flex items-center gap-2 text-sm">
            <Check size={16} className="text-[var(--ok)]" /> Recherche par le sens avec <b>{status.embedding_model}</b>, sur
            votre processeur.
            {status.pending_embeddings > 0 && <span className="text-[var(--muted)]">{status.pending_embeddings} passages en attente de vecteurs.</span>}
          </p>
        ) : downloads[dlKey] ? (
          <DownloadRow id={dlKey} state={downloads[dlKey]} />
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-sm text-[var(--muted)]">
              Sans modèle d'embeddings, la recherche se fait au mot près. Installez-le pour chercher par le sens (≈ {formatBytes(639150592)}).
            </p>
            <Button variant="primary" onClick={installEmbeddings} disabled={!status.recommended}>
              <Download size={16} /> Installer
            </Button>
          </div>
        )}
      </Card>

      <div className="flex items-center justify-between">
        <p className="text-sm text-[var(--muted)]">
          Textes, PDF (même scannés, par reconnaissance de caractères), Word, Excel, PowerPoint et OpenDocument. Les modifications sont prises en compte automatiquement.
        </p>
        <Button onClick={add}>
          <FolderPlus size={16} /> Indexer un dossier
        </Button>
      </div>
      {status.sources.length === 0 && <p className="text-sm text-[var(--muted)]">Aucun dossier indexé.</p>}
      {status.sources.map((s) => {
        const p = progress[s.id];
        const busy = s.status === "scanning" || s.status === "embedding";
        return (
          <Card key={s.id} className="space-y-2 py-3">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="truncate font-medium">{s.path}</div>
                <div className="text-xs text-[var(--muted)]">
                  {s.files} fichiers{s.failed_files > 0 && ` (${s.failed_files} illisibles)`} · {s.chunks} passages
                  {status.embedding_model && ` · ${s.embedded_chunks} vecteurs`}
                  {s.last_scan && ` · mis à jour ${new Date(s.last_scan).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" })}`}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <Badge tone={s.status === "idle" ? "ok" : s.status === "error" ? "neutral" : "accent"}>
                  {busy && <Spinner className="mr-1 size-3" />}
                  {STATUS_LABEL[s.status] ?? s.status}
                </Badge>
                <Button variant="ghost" aria-label="Réindexer" disabled={busy} onClick={() => act(() => api.reindexSource(s.id))}>
                  <RefreshCw size={15} />
                </Button>
                <Button
                  variant="danger"
                  aria-label="Retirer de l'index"
                  onClick={() => window.confirm(`Retirer ${s.path} de l'index ? Les fichiers ne sont pas touchés.`) && act(() => api.removeIndexSource(s.id))}
                >
                  <Trash2 size={15} />
                </Button>
              </div>
            </div>
            {s.status === "scanning" && p && (
              <p className="truncate text-xs text-[var(--muted)]">
                {p.done}/{p.total} · {p.path}
              </p>
            )}
            {s.error && <ErrorNote message={s.error} />}
          </Card>
        );
      })}

      {status.sources.length > 0 && (
        <Card className="space-y-3">
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              search();
            }}
          >
            <input className={`${input} flex-1`} placeholder="Essayer une recherche dans vos documents" value={query} onChange={(e) => setQuery(e.target.value)} />
            <Button type="submit" disabled={!query.trim() || searching}>
              {searching ? <Spinner /> : <FileSearch size={16} />} Chercher
            </Button>
          </form>
          {hits && hits.length === 0 && <p className="text-sm text-[var(--muted)]">Aucun passage trouvé.</p>}
          {hits?.map((h, i) => (
            <div key={i} className="rounded-lg border border-[var(--border)] p-3 text-sm">
              <div className="truncate font-medium">
                {h.path.split("\\").pop()} {h.location && <span className="text-[var(--muted)]">· {h.location}</span>}
              </div>
              <div className="truncate text-xs text-[var(--muted)]">{h.path}</div>
              <p className="mt-1 line-clamp-4 whitespace-pre-wrap">{h.text}</p>
            </div>
          ))}
        </Card>
      )}
    </div>
  );
}

function Skills({
  settings,
  onSettings,
  onError,
}: {
  settings: MemorySettings;
  onSettings: (p: Partial<MemorySettings>) => void;
  onError: (e: string | null) => void;
}) {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [proposals, setProposals] = useState<SkillProposal[]>([]);
  const [edit, setEdit] = useState<(Skill & { proposalId?: number; isNew?: boolean }) | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, p] = await Promise.all([api.listSkills(), api.skillProposals()]);
      setSkills(s);
      setProposals(p);
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  useEffect(() => {
    load();
    return api.onAppEvent((e) => e.type === "skill_proposed" && load());
  }, [load]);

  const act = async (job: () => Promise<unknown>) => {
    onError(null);
    try {
      await job();
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  const save = () =>
    edit &&
    act(async () => {
      if (edit.proposalId != null) await api.acceptSkill(edit.proposalId, edit.name, edit.description, edit.body);
      else await api.saveSkill(edit.name, edit.description, edit.body);
      setEdit(null);
    });

  return (
    <div className="space-y-4">
      <Card className="flex flex-wrap items-center justify-between gap-3">
        <Toggle
          checked={settings.learn_skills}
          onChange={(learn_skills) => onSettings({ learn_skills })}
          label="Apprendre des tâches réussies"
          hint="Après une tâche de fond réussie, Claudio peut proposer une procédure réutilisable. Elle ne sert qu'une fois validée par vous."
        />
        <div className="flex gap-2">
          <Button onClick={() => api.openFolder("skills")}>
            <FolderOpen size={16} /> Dossier
          </Button>
          <Button onClick={() => setEdit({ name: "", description: "", body: "", isNew: true })}>
            <Plus size={16} /> Nouvelle
          </Button>
        </div>
      </Card>

      {edit && (
        <Card className="space-y-2">
          <div className="flex gap-2">
            <input className={`${input} w-56 font-mono`} placeholder="nom-de-la-competence" value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} />
            <input className={`${input} flex-1`} placeholder="Quand l'utiliser (une phrase)" value={edit.description} onChange={(e) => setEdit({ ...edit, description: e.target.value })} />
          </div>
          <textarea
            className={`${input} min-h-48 w-full font-mono text-sm`}
            placeholder="Procédure en Markdown : étapes, outils, pièges à éviter"
            value={edit.body}
            onChange={(e) => setEdit({ ...edit, body: e.target.value })}
          />
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setEdit(null)}>
              Annuler
            </Button>
            <Button variant="primary" disabled={!edit.name.trim() || !edit.body.trim()} onClick={save}>
              {edit.proposalId != null ? "Valider la compétence" : "Enregistrer"}
            </Button>
          </div>
        </Card>
      )}

      {proposals.length > 0 && (
        <section className="space-y-2">
          <h2 className="flex items-center gap-2 font-medium">
            <Lightbulb size={17} className="text-amber-500" /> Propositions à valider
          </h2>
          {proposals.map((p) => (
            <Card key={p.id} className="space-y-2 border-amber-500/50 py-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="font-mono font-medium">{p.name}</div>
                  <div className="text-sm text-[var(--muted)]">{p.description}</div>
                  <div className="text-xs text-[var(--muted)]">Tirée de la tâche « {p.origin} »</div>
                </div>
                <div className="flex shrink-0 gap-2">
                  <Button onClick={() => setEdit({ name: p.name, description: p.description, body: p.body, proposalId: p.id })}>
                    <Pencil size={15} /> Relire et valider
                  </Button>
                  <Button variant="danger" aria-label="Refuser" onClick={() => act(() => api.rejectSkill(p.id))}>
                    <X size={16} />
                  </Button>
                </div>
              </div>
            </Card>
          ))}
        </section>
      )}

      <section className="space-y-2">
        <h2 className="font-medium">Compétences validées</h2>
        {skills.length === 0 && <p className="text-sm text-[var(--muted)]">Aucune compétence pour l'instant.</p>}
        {skills.map((s) => (
          <Card key={s.name} className="space-y-2 py-3">
            <div className="flex items-start justify-between gap-3">
              <button className="min-w-0 text-left" onClick={() => setOpen(open === s.name ? null : s.name)}>
                <div className="font-mono font-medium">{s.name}</div>
                <div className="text-sm text-[var(--muted)]">{s.description}</div>
              </button>
              <div className="flex shrink-0 gap-1">
                <Button variant="ghost" aria-label="Modifier" onClick={() => setEdit({ ...s })}>
                  <Pencil size={15} />
                </Button>
                <Button
                  variant="danger"
                  aria-label="Supprimer"
                  onClick={() => window.confirm(`Supprimer la compétence ${s.name} ?`) && act(() => api.deleteSkill(s.name))}
                >
                  <Trash2 size={15} />
                </Button>
              </div>
            </div>
            {open === s.name && (
              <div className="prose-claudio rounded-lg bg-[var(--panel-2)] p-3 text-sm">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{s.body}</ReactMarkdown>
              </div>
            )}
          </Card>
        ))}
      </section>
    </div>
  );
}
