// Agents : tâches de fond (graphe d'étapes en direct), planifications et catalogue d'agents.

import clsx from "clsx";
import {
  AlertTriangle,
  ArrowRight,
  Ban,
  Bot,
  CalendarClock,
  Check,
  CircleDashed,
  FolderOpen,
  Copy,
  Hourglass,
  Pencil,
  Plus,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldQuestion,
  Square,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  api,
  formatUsd,
  stepStatusLabels,
  taskStatusLabels,
  type ActiveModel,
  type AgentDef,
  type AgentsView,
  type Config,
  type ScheduleView,
  type Status,
  type StepStatus,
  type Task,
  type TaskDetail,
  type TaskStatus,
} from "../api";
import { Badge, Button, Card, ErrorNote, PageHeader, Spinner } from "../ui";

type Tab = "tasks" | "schedules" | "agents";
const input = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";

const PRESETS: { label: string; cron: string }[] = [
  { label: "Tous les jours à 8 h", cron: "0 8 * * *" },
  { label: "Du lundi au vendredi à 8 h", cron: "0 8 * * 1-5" },
  { label: "Chaque lundi à 9 h", cron: "0 9 * * 1" },
  { label: "Toutes les heures", cron: "0 * * * *" },
  { label: "Le 1er du mois à 7 h", cron: "0 7 1 * *" },
];

export function cronLabel(cron: string): string {
  return PRESETS.find((p) => p.cron === cron)?.label ?? `cron « ${cron} »`;
}

function statusTone(s: TaskStatus | StepStatus): "ok" | "accent" | "neutral" {
  if (s === "done") return "ok";
  if (s === "running" || s === "planning" || s === "waiting") return "accent";
  return "neutral";
}

function StepIcon({ status }: { status: StepStatus }) {
  const cls = "shrink-0";
  switch (status) {
    case "done":
      return <Check size={15} className={`${cls} text-[var(--ok)]`} />;
    case "running":
      return <Spinner className="size-3.5 text-[var(--accent)]" />;
    case "waiting":
      return <ShieldQuestion size={15} className={`${cls} text-amber-500`} />;
    case "failed":
      return <AlertTriangle size={15} className={`${cls} text-[var(--danger)]`} />;
    case "skipped":
    case "cancelled":
      return <Ban size={15} className={`${cls} text-[var(--muted)]`} />;
    default:
      return <CircleDashed size={15} className={`${cls} text-[var(--muted)]`} />;
  }
}

function modeOptions(agents: AgentsView | null) {
  return [
    { value: "plan", label: "Planifier automatiquement (plusieurs agents)" },
    ...(agents?.agents ?? [])
      .filter((a) => a.id !== "planner")
      .map((a) => ({ value: `agent:${a.id}`, label: `Un seul agent : ${a.name}` })),
  ];
}

export function Agents() {
  const [tab, setTab] = useState<Tab>("tasks");
  const [agents, setAgents] = useState<AgentsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.listAgents().then(setAgents, (e) => setError(String(e)));
  }, []);

  return (
    <div className="mx-auto max-w-6xl space-y-5 px-6 py-8">
      <PageHeader
        title="Agents"
        subtitle="Confiez un objectif : il est découpé en étapes, réparties entre agents spécialisés et exécutées en arrière-plan."
      />
      <div className="flex gap-1 border-b border-[var(--border)]">
        {(
          [
            ["tasks", "Tâches"],
            ["schedules", "Planifications"],
            ["agents", "Catalogue d'agents"],
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
      {tab === "tasks" && <Tasks agents={agents} onError={setError} />}
      {tab === "schedules" && <Schedules agents={agents} onError={setError} />}
      {tab === "agents" && <Catalog agents={agents} onChange={setAgents} onReload={async () => setAgents(await api.listAgents(true))} />}
    </div>
  );
}

function Tasks({ agents, onError }: { agents: AgentsView | null; onError: (e: string | null) => void }) {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<TaskDetail | null>(null);
  const [live, setLive] = useState<Record<string, string>>({});
  const [objective, setObjective] = useState("");
  const [mode, setMode] = useState("plan");
  const refreshTimer = useRef<number | null>(null);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  const refresh = useCallback(async () => {
    try {
      const list = await api.listTasks();
      setTasks(list);
      const id = selectedRef.current ?? list[0]?.id ?? null;
      if (id) {
        setSelected(id);
        setDetail(await api.taskDetail(id));
      }
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  useEffect(() => {
    refresh();
    // Les mises à jour arrivent en rafale : on regroupe les rechargements.
    return api.onAppEvent((e) => {
      if (e.type === "step_output") {
        setLive((l) => ({ ...l, [`${e.task_id}/${e.step_id}`]: (l[`${e.task_id}/${e.step_id}`] ?? "") + e.text }));
        return;
      }
      if (refreshTimer.current == null) {
        refreshTimer.current = window.setTimeout(() => {
          refreshTimer.current = null;
          refresh();
        }, 250);
      }
    });
  }, [refresh]);

  const open = async (id: string) => {
    setSelected(id);
    setDetail(await api.taskDetail(id));
  };

  const start = async () => {
    onError(null);
    try {
      const t = await api.startTask(objective, mode);
      setObjective("");
      setSelected(t.id);
      await refresh();
    } catch (e) {
      onError(String(e));
    }
  };

  const act = async (job: () => Promise<unknown>) => {
    onError(null);
    try {
      await job();
      await refresh();
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="space-y-5">
      <Card className="space-y-3">
        <textarea
          className={`${input} min-h-20 w-full`}
          placeholder="Objectif, ex. « Compare les trois dernières factures d'électricité de D:\Documents\Factures et propose comment réduire la consommation »"
          value={objective}
          onChange={(e) => setObjective(e.target.value)}
        />
        <div className="flex flex-wrap items-center justify-between gap-3">
          <select className={input} value={mode} onChange={(e) => setMode(e.target.value)}>
            {modeOptions(agents).map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <Button variant="primary" disabled={!objective.trim()} onClick={start}>
            <Play size={16} /> Lancer en arrière-plan
          </Button>
        </div>
      </Card>

      <div className="grid gap-5 lg:grid-cols-[18rem_1fr]">
        <div className="space-y-1.5">
          {tasks.length === 0 && <p className="text-sm text-[var(--muted)]">Aucune tâche pour l'instant.</p>}
          {tasks.map((t) => (
            <button
              key={t.id}
              onClick={() => open(t.id)}
              className={clsx(
                "w-full rounded-lg border p-3 text-left",
                t.id === selected ? "border-[var(--accent)] bg-[var(--accent-soft)]/30" : "border-[var(--border)] bg-[var(--panel)] hover:bg-[var(--panel-2)]",
              )}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="truncate font-medium">{t.title}</span>
                {t.schedule_id != null && <CalendarClock size={14} className="shrink-0 text-[var(--muted)]" />}
              </div>
              <div className="mt-1 flex items-center gap-2 text-xs text-[var(--muted)]">
                <Badge tone={statusTone(t.status)}>{taskStatusLabels[t.status]}</Badge>
                {t.steps_total > 0 && (
                  <span>
                    {t.steps_done}/{t.steps_total} étapes
                  </span>
                )}
                <span className="ml-auto">{new Date(t.created_at).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" })}</span>
              </div>
            </button>
          ))}
        </div>
        {detail ? <Detail detail={detail} live={live} act={act} agents={agents} /> : <div />}
      </div>
    </div>
  );
}

function Detail({
  detail,
  live,
  act,
  agents,
}: {
  detail: TaskDetail;
  live: Record<string, string>;
  act: (job: () => Promise<unknown>) => Promise<void>;
  agents: AgentsView | null;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const active = detail.status === "planning" || detail.status === "running";
  const byId = Object.fromEntries(detail.steps.map((s) => [s.step_id, s]));
  const agentName = (id: string) => agents?.agents.find((a) => a.id === id)?.name ?? id;
  const step = open ? byId[open] : null;

  return (
    <Card className="min-w-0 space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h2 className="text-lg font-semibold">{detail.title}</h2>
            <Badge tone={statusTone(detail.status)}>{taskStatusLabels[detail.status]}</Badge>
          </div>
          <p className="mt-1 text-sm whitespace-pre-wrap text-[var(--muted)]">{detail.objective}</p>
          <p className="mt-1 text-xs text-[var(--muted)]">
            {detail.tokens.toLocaleString("fr-FR")} jetons
            {detail.cost_usd > 0 && ` · ${formatUsd(detail.cost_usd)}`}
          </p>
        </div>
        <div className="flex gap-2">
          {active ? (
            <Button onClick={() => act(() => api.cancelTask(detail.id))}>
              <Square size={15} /> Arrêter
            </Button>
          ) : (
            <>
              {detail.status !== "done" && (
                <Button onClick={() => act(() => api.resumeTask(detail.id))}>
                  <RotateCcw size={15} /> Reprendre
                </Button>
              )}
              <Button
                variant="danger"
                aria-label="Supprimer la tâche"
                onClick={() => window.confirm("Supprimer cette tâche et ses résultats ?") && act(() => api.deleteTask(detail.id))}
              >
                <Trash2 size={16} />
              </Button>
            </>
          )}
        </div>
      </div>

      {detail.status === "planning" && detail.steps.length === 0 && (
        <p className="flex items-center gap-2 text-sm text-[var(--muted)]">
          <Spinner /> Le planificateur découpe l'objectif en étapes…
        </p>
      )}

      {detail.levels.length > 0 && (
        <div className="flex items-stretch gap-2 overflow-x-auto pb-2" role="list" aria-label="Étapes de la tâche">
          {detail.levels.map((level, i) => (
            <div key={i} className="flex items-center gap-2">
              {i > 0 && <ArrowRight size={16} className="shrink-0 text-[var(--muted)]" />}
              <div className="flex flex-col gap-2">
                {level.map((id) => {
                  const s = byId[id];
                  if (!s) return null;
                  return (
                    <button
                      key={id}
                      role="listitem"
                      onClick={() => setOpen(open === id ? null : id)}
                      className={clsx(
                        "w-52 rounded-lg border p-2.5 text-left text-sm",
                        open === id ? "border-[var(--accent)]" : "border-[var(--border)] hover:bg-[var(--panel-2)]",
                        s.status === "waiting" && "border-amber-500",
                      )}
                    >
                      <div className="flex items-center gap-1.5">
                        <StepIcon status={s.status} />
                        <span className="truncate font-medium">{s.title}</span>
                      </div>
                      <div className="mt-1 flex items-center gap-1 text-xs text-[var(--muted)]">
                        <Bot size={12} /> {agentName(s.agent)}
                      </div>
                      <div className="text-xs text-[var(--muted)]">{stepStatusLabels[s.status]}</div>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      )}

      {step && (
        <div className="space-y-3 rounded-lg border border-[var(--border)] p-3">
          <div className="flex items-center justify-between gap-2">
            <h3 className="font-medium">
              {step.title} · {agentName(step.agent)}
            </h3>
            {step.model && <span className="text-xs text-[var(--muted)]">{step.model}</span>}
          </div>
          <p className="text-sm whitespace-pre-wrap text-[var(--muted)]">{step.instructions}</p>
          {step.log && (
            <pre className="max-h-40 overflow-auto rounded bg-[var(--panel-2)] p-2 font-mono text-xs whitespace-pre-wrap">{step.log}</pre>
          )}
          {step.error && <ErrorNote message={step.error} />}
          {(step.output ?? live[`${detail.id}/${step.step_id}`]) && (
            <div className="prose-claudio rounded-lg border border-[var(--border)] p-3 text-sm">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{step.output ?? live[`${detail.id}/${step.step_id}`] ?? ""}</ReactMarkdown>
            </div>
          )}
        </div>
      )}

      {detail.error && <ErrorNote message={detail.error} />}
      {detail.result && !active && (
        <div className="space-y-2">
          <h3 className="flex items-center gap-2 font-medium">
            <Check size={16} className="text-[var(--ok)]" /> Résultat
          </h3>
          <div className="prose-claudio rounded-lg bg-[var(--panel-2)] p-4">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{detail.result}</ReactMarkdown>
          </div>
        </div>
      )}
    </Card>
  );
}

function Schedules({ agents, onError }: { agents: AgentsView | null; onError: (e: string | null) => void }) {
  const [list, setList] = useState<ScheduleView[]>([]);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [objective, setObjective] = useState("");
  const [mode, setMode] = useState("plan");
  const [preset, setPreset] = useState(PRESETS[0].cron);
  const [custom, setCustom] = useState("");

  const load = useCallback(async () => setList(await api.listSchedules()), []);
  useEffect(() => {
    load().catch((e) => onError(String(e)));
  }, [load, onError]);

  const act = async (job: () => Promise<unknown>) => {
    onError(null);
    try {
      await job();
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  const add = () =>
    act(async () => {
      await api.addSchedule(title, objective, mode, preset === "custom" ? custom : preset);
      setAdding(false);
      setTitle("");
      setObjective("");
    });

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-sm text-[var(--muted)]">
          Les tâches planifiées s'exécutent même fenêtre fermée, tant que Claudio tourne dans la zone de notification.
        </p>
        {!adding && (
          <Button onClick={() => setAdding(true)}>
            <CalendarClock size={16} /> Nouvelle planification
          </Button>
        )}
      </div>
      {adding && (
        <Card className="space-y-3">
          <input className={`${input} w-full`} placeholder="Nom (facultatif)" value={title} onChange={(e) => setTitle(e.target.value)} />
          <textarea
            className={`${input} min-h-20 w-full`}
            placeholder="Objectif, ex. « Fais une revue de presse sur l'IA locale et enregistre-la dans mon espace de travail »"
            value={objective}
            onChange={(e) => setObjective(e.target.value)}
          />
          <div className="flex flex-wrap gap-2">
            <select className={input} value={mode} onChange={(e) => setMode(e.target.value)}>
              {modeOptions(agents).map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
            <select className={input} value={preset} onChange={(e) => setPreset(e.target.value)}>
              {PRESETS.map((p) => (
                <option key={p.cron} value={p.cron}>
                  {p.label}
                </option>
              ))}
              <option value="custom">Personnalisé (cron)…</option>
            </select>
            {preset === "custom" && (
              <input
                className={`${input} font-mono`}
                placeholder="min heure jour mois jour-sem."
                value={custom}
                onChange={(e) => setCustom(e.target.value)}
              />
            )}
          </div>
          <p className="text-xs text-[var(--muted)]">
            Une tâche planifiée tourne sans vous : les demandes d'autorisation vous seront notifiées. Accordez à l'avance les
            dossiers nécessaires dans « Outils et accès ».
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setAdding(false)}>
              Annuler
            </Button>
            <Button variant="primary" disabled={!objective.trim() || (preset === "custom" && !custom.trim())} onClick={add}>
              Créer
            </Button>
          </div>
        </Card>
      )}
      {list.length === 0 && !adding && <p className="text-sm text-[var(--muted)]">Aucune planification.</p>}
      {list.map((s) => (
        <Card key={s.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
          <div className="min-w-0">
            <div className="font-medium">{s.title}</div>
            <div className="text-sm text-[var(--muted)]">
              {cronLabel(s.cron)}
              {s.next_run && ` · prochaine : ${new Date(s.next_run).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" })}`}
              {s.last_run && ` · dernière : ${new Date(s.last_run).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" })}`}
            </div>
            <div className="truncate text-xs text-[var(--muted)]">{s.objective}</div>
          </div>
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-1.5 text-sm">
              <input
                type="checkbox"
                className="accent-[var(--accent)]"
                checked={s.enabled}
                onChange={(e) => act(() => api.setScheduleEnabled(s.id, e.target.checked))}
              />
              Activée
            </label>
            <Button onClick={() => act(() => api.runScheduleNow(s.id))}>
              <Play size={15} /> Lancer
            </Button>
            <Button
              variant="danger"
              aria-label="Supprimer"
              onClick={() => window.confirm(`Supprimer la planification « ${s.title} » ?`) && act(() => api.deleteSchedule(s.id))}
            >
              <Trash2 size={16} />
            </Button>
          </div>
        </Card>
      ))}
    </div>
  );
}

const modelValue = (m: ActiveModel | undefined) => (!m ? "" : m.source === "local" ? `local:${m.id}` : `ollama:${m.name}`);
const modelFromValue = (v: string): ActiveModel | null =>
  v.startsWith("local:") ? { source: "local", id: v.slice(6) } : v.startsWith("ollama:") ? { source: "ollama", name: v.slice(7) } : null;

function Catalog({
  agents,
  onChange,
  onReload,
}: {
  agents: AgentsView | null;
  onChange: (v: AgentsView) => void;
  onReload: () => Promise<void>;
}) {
  const [editing, setEditing] = useState<{ agent: AgentDef; create: boolean } | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.status().then(setStatus, (e) => setError(String(e)));
    api.config().then(setConfig, (e) => setError(String(e)));
  }, []);
  if (!agents) return <Spinner />;
  const chatModels = (status?.installed_models ?? []).filter((m) => !status?.embedding_models.includes(m.key));
  const activeName = !status?.active_model
    ? "aucun"
    : status.active_model.source === "local"
      ? status.active_model.id
      : status.active_model.name;
  const choose = async (agentId: string, value: string) => {
    try {
      await api.setAgentModel(agentId, modelFromValue(value));
      setConfig(await api.config());
    } catch (e) {
      setError(String(e));
    }
  };
  const remove = async (a: AgentDef) => {
    const question = a.overrides_builtin
      ? `Rétablir l'agent « ${a.name} » tel que fourni par Claudio ? Vos modifications seront perdues.`
      : `Supprimer l'agent « ${a.name} » ?`;
    if (!window.confirm(question)) return;
    try {
      onChange(await api.deleteAgent(a.id));
      setConfig(await api.config());
    } catch (e) {
      setError(String(e));
    }
  };
  if (editing)
    return (
      <AgentEditor
        initial={editing.agent}
        create={editing.create}
        taken={agents.agents.map((a) => a.id)}
        onCancel={() => setEditing(null)}
        onSaved={(v) => {
          onChange(v);
          setEditing(null);
        }}
      />
    );
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-[var(--muted)]">
          Consignes, outils et modèle de chaque agent. Les agents fournis peuvent être modifiés puis rétablis ; les vôtres sont
          enregistrés en YAML dans le dossier des agents.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" onClick={() => setEditing({ agent: NEW_AGENT, create: true })}>
            <Plus size={16} /> Nouvel agent
          </Button>
          <Button onClick={() => api.openFolder("agents")}>
            <FolderOpen size={16} /> Dossier des agents
          </Button>
          <Button onClick={onReload}>
            <RefreshCw size={16} /> Recharger
          </Button>
        </div>
      </div>
      <p className="text-sm text-[var(--muted)]">
        Chaque agent peut utiliser son propre modèle local. Plusieurs modèles restent chargés ensemble tant qu'ils tiennent dans
        la mémoire vidéo ; sinon le moins utilisé est déchargé quand il est libre.
      </p>
      {error && <ErrorNote message={error} />}
      {agents.errors.map(([file, why]) => (
        <ErrorNote key={file} message={`${file} ignoré : ${why}`} />
      ))}
      <div className="grid gap-3 sm:grid-cols-2">
        {agents.agents.map((a) => (
          <Card key={a.id} className="space-y-2">
            <div className="flex items-center gap-2">
              <Bot size={18} className="text-[var(--accent)]" />
              <span className="font-medium">{a.name}</span>
              <span className="font-mono text-xs text-[var(--muted)]">{a.id}</span>
              {a.overrides_builtin ? (
                <Badge tone="accent">modifié</Badge>
              ) : (
                !a.builtin && <Badge tone="accent">personnalisé</Badge>
              )}
              {a.model !== "auto" && <Badge>{a.model === "local" ? "local" : "cloud"}</Badge>}
            </div>
            <p className="text-sm text-[var(--muted)]">{a.description}</p>
            <div className="flex flex-wrap gap-1">
              {a.tools.length === 0 ? (
                <span className="text-xs text-[var(--muted)]">Aucun outil</span>
              ) : (
                a.tools.map((t) => (
                  <span key={t} className="rounded bg-[var(--panel-2)] px-1.5 py-0.5 font-mono text-xs">
                    {t}
                  </span>
                ))
              )}
            </div>
            <div className="flex items-center gap-1 text-xs text-[var(--muted)]">
              <Hourglass size={12} /> {a.max_steps} étapes d'outils au plus
            </div>
            {config && (
              <label className="flex items-center gap-2 text-sm">
                <span className="shrink-0 text-[var(--muted)]">Modèle local</span>
                <select
                  className={clsx(input, "min-w-0 flex-1 py-1 text-sm")}
                  value={modelValue(config.agent_models[a.id])}
                  onChange={(e) => choose(a.id, e.target.value)}
                >
                  <option value="">Modèle actif ({activeName})</option>
                  {chatModels.map((m) => (
                    <option key={m.key} value={`local:${m.key}`}>
                      {m.key}
                    </option>
                  ))}
                  {(status?.ollama_models ?? []).map((m) => (
                    <option key={m.name} value={`ollama:${m.name}`}>
                      {m.name} (Ollama)
                    </option>
                  ))}
                </select>
              </label>
            )}
            <div className="flex flex-wrap gap-1 pt-1">
              <Button variant="ghost" className="px-2 py-1" onClick={() => setEditing({ agent: a, create: false })}>
                <Pencil size={14} /> Modifier
              </Button>
              <Button
                variant="ghost"
                className="px-2 py-1"
                onClick={() =>
                  setEditing({
                    agent: { ...a, id: `${a.id}-copie`, name: `${a.name} (copie)`, builtin: false, file: null, overrides_builtin: false },
                    create: true,
                  })
                }
              >
                <Copy size={14} /> Dupliquer
              </Button>
              {a.file && (
                <Button variant="danger" className="px-2 py-1" onClick={() => remove(a)}>
                  {a.overrides_builtin ? <RotateCcw size={14} /> : <Trash2 size={14} />}
                  {a.overrides_builtin ? "Rétablir l'original" : "Supprimer"}
                </Button>
              )}
            </div>
          </Card>
        ))}
      </div>
    </div>
  );
}

const NEW_AGENT: AgentDef = {
  id: "",
  name: "",
  description: "",
  model: "auto",
  tools: ["list_directory", "read_file"],
  max_steps: 12,
  thinking: null,
  system_prompt: "Tu es … de Claudio. Tu réponds en français.",
  builtin: false,
  file: null,
  overrides_builtin: false,
};

function AgentEditor({
  initial,
  create,
  taken,
  onCancel,
  onSaved,
}: {
  initial: AgentDef;
  create: boolean;
  taken: string[];
  onCancel: () => void;
  onSaved: (v: AgentsView) => void;
}) {
  const [a, setA] = useState<AgentDef>(initial);
  const [tools, setTools] = useState<[string, string][] | null>(null);
  const [extra, setExtra] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.availableTools().then(setTools, () => setTools([]));
  }, []);
  const set = (patch: Partial<AgentDef>) => setA((x) => ({ ...x, ...patch }));
  const known = new Set((tools ?? []).map(([n]) => n));
  // Motifs (`web_*`, `*`) et outils absents (serveur MCP arrêté) : saisis à la main.
  const extraText = extra ?? a.tools.filter((t) => !known.has(t)).join(", ");
  const toggle = (name: string, on: boolean) => set({ tools: on ? [...a.tools, name] : a.tools.filter((t) => t !== name) });
  const idError = !create
    ? null
    : !/^[A-Za-z0-9_-]+$/.test(a.id)
      ? "Lettres sans accents, chiffres, - et _ uniquement."
      : taken.includes(a.id)
        ? "Cet identifiant est déjà pris."
        : null;
  const save = async () => {
    const extras = extraText
      .split(/[,\s]+/)
      .map((t) => t.trim())
      .filter(Boolean);
    const agent = { ...a, tools: [...new Set([...a.tools.filter((t) => known.has(t)), ...extras])] };
    setSaving(true);
    setError(null);
    try {
      onSaved(await api.saveAgent(agent, create));
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };
  const field = "flex flex-col gap-1 text-sm";
  const hint = "text-xs text-[var(--muted)]";
  return (
    <Card className="space-y-4">
      <div className="flex items-center gap-2">
        <Bot size={18} className="text-[var(--accent)]" />
        <h2 className="font-medium">{create ? "Nouvel agent" : `Modifier « ${initial.name} »`}</h2>
      </div>
      {!create && initial.builtin && (
        <p className={hint}>
          Agent fourni par Claudio : vos modifications sont enregistrées à part et peuvent être annulées avec « Rétablir
          l'original ».
        </p>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <label className={field}>
          <span>Identifiant</span>
          <input
            className={clsx(input, "font-mono")}
            value={a.id}
            disabled={!create}
            placeholder="traducteur"
            onChange={(e) => set({ id: e.target.value.trim() })}
          />
          {idError && a.id && <span className="text-xs text-[var(--danger)]">{idError}</span>}
        </label>
        <label className={field}>
          <span>Nom</span>
          <input className={input} value={a.name} placeholder="Traducteur" onChange={(e) => set({ name: e.target.value })} />
        </label>
      </div>
      <label className={field}>
        <span>Description</span>
        <input
          className={input}
          value={a.description}
          placeholder="Traduit des documents entre le français et l'anglais."
          onChange={(e) => set({ description: e.target.value })}
        />
        <span className={hint}>Le planificateur s'en sert pour confier les étapes : dites à quoi l'agent est bon.</span>
      </label>
      <label className={field}>
        <span>Consignes</span>
        <textarea
          className={clsx(input, "min-h-40 font-mono text-xs leading-relaxed")}
          value={a.system_prompt}
          onChange={(e) => set({ system_prompt: e.target.value })}
        />
      </label>
      <div className="grid gap-3 sm:grid-cols-3">
        <label className={field}>
          <span>Où l'exécuter</span>
          <select className={input} value={a.model} onChange={(e) => set({ model: e.target.value as AgentDef["model"] })}>
            <option value="auto">Selon le routage général</option>
            <option value="local">Toujours en local</option>
            <option value="cloud">De préférence dans le cloud</option>
          </select>
        </label>
        <label className={field}>
          <span>Réflexion</span>
          <select
            className={input}
            value={a.thinking === null ? "" : String(a.thinking)}
            onChange={(e) => set({ thinking: e.target.value === "" ? null : e.target.value === "true" })}
          >
            <option value="">Réglage général</option>
            <option value="true">Activée</option>
            <option value="false">Désactivée</option>
          </select>
        </label>
        <label className={field}>
          <span>Étapes d'outils au plus</span>
          <input
            type="number"
            min={1}
            max={50}
            className={input}
            value={a.max_steps}
            onChange={(e) => set({ max_steps: Number(e.target.value) })}
          />
        </label>
      </div>
      <div className="space-y-2">
        <div className="text-sm">Outils autorisés</div>
        {!tools ? (
          <Spinner />
        ) : (
          <div className="grid gap-1 sm:grid-cols-2">
            {tools.map(([name, description]) => (
              <label key={name} className="flex items-start gap-2 text-sm" title={description}>
                <input type="checkbox" className="mt-1" checked={a.tools.includes(name)} onChange={(e) => toggle(name, e.target.checked)} />
                <span className="font-mono text-xs leading-6">{name}</span>
              </label>
            ))}
          </div>
        )}
        <label className={field}>
          <span className={hint}>
            Autres motifs, séparés par des virgules : <code>web_*</code>, <code>mcp__*</code>, ou <code>*</code> pour tous les outils.
            Les autorisations de la page « Outils et accès » s'appliquent toujours.
          </span>
          <input className={clsx(input, "font-mono text-xs")} value={extraText} onChange={(e) => setExtra(e.target.value)} />
        </label>
      </div>
      <ErrorNote message={error} />
      <div className="flex justify-end gap-2">
        <Button onClick={onCancel}>Annuler</Button>
        <Button
          variant="primary"
          disabled={saving || !!idError || !a.name.trim() || !a.system_prompt.trim() || a.max_steps < 1 || a.max_steps > 50}
          onClick={save}
        >
          <Check size={16} /> Enregistrer
        </Button>
      </div>
    </Card>
  );
}
