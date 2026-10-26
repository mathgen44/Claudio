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
  Hourglass,
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
  type AgentsView,
  type ScheduleView,
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
      {tab === "agents" && <Catalog agents={agents} onReload={async () => setAgents(await api.listAgents(true))} />}
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
          placeholder="Objectif, ex. « Compare les trois dernières factures d'électricité de D:\\Documents\\Factures et propose comment réduire la consommation »"
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

function Catalog({ agents, onReload }: { agents: AgentsView | null; onReload: () => Promise<void> }) {
  if (!agents) return <Spinner />;
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-[var(--muted)]">
          Chaque agent est un fichier YAML : consignes, outils autorisés, modèle préféré. Ajoutez les vôtres dans le dossier des
          agents (un exemple y est expliqué).
        </p>
        <div className="flex gap-2">
          <Button onClick={() => api.openFolder("agents")}>
            <FolderOpen size={16} /> Dossier des agents
          </Button>
          <Button onClick={onReload}>
            <RefreshCw size={16} /> Recharger
          </Button>
        </div>
      </div>
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
              {!a.builtin && <Badge tone="accent">personnalisé</Badge>}
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
          </Card>
        ))}
      </div>
    </div>
  );
}
