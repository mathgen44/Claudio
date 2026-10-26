// Conversations : liste des sessions, fil de messages en continu, saisie.

import { Brain, ChevronRight, Cloud, Cpu, MessageSquarePlus, Pencil, Send, Square, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  activeLabel,
  api,
  formatUsd,
  routingLabels,
  type ApprovalRequest,
  type Config,
  type Message,
  type RoutingMode,
  type Session,
  type StreamEvent,
  type ToolTrace,
} from "../api";
import { ApprovalDialog } from "../components/ApprovalDialog";
import { ToolCard, type ToolStep } from "../components/ToolCard";
import { useStore } from "../store";
import { Button, ErrorNote, Spinner } from "../ui";

type Route = Extract<StreamEvent, { type: "route" }>;

interface Pending {
  user: string;
  reasoning: string;
  content: string;
  route?: Route;
  tools: ToolStep[];
}

function applyEvent(p: Pending, e: StreamEvent): Pending {
  switch (e.type) {
    // Un nouvel itinéraire (repli) repart de zéro.
    case "route":
      return { ...p, route: e, content: "", reasoning: "", tools: [] };
    case "token":
      return { ...p, content: p.content + e.text };
    case "reasoning":
      return { ...p, reasoning: p.reasoning + e.text };
    case "tool_call":
      // Le texte qui précède un appel d'outil reste affiché, puis la suite continue en dessous.
      return { ...p, tools: [...p.tools, { id: e.id, name: e.name, arguments: e.arguments }] };
    case "tool_result":
      return {
        ...p,
        tools: p.tools.map((t) => (t.id === e.id && t.result == null ? { ...t, result: e.content, is_error: e.is_error } : t)),
      };
    case "approval":
      return p;
  }
}

/** Regroupe l'historique : les traces d'outils sont rattachées à la réponse qui suit. */
function groupHistory(messages: Message[]): { message: Message; tools: ToolStep[] }[] {
  const out: { message: Message; tools: ToolStep[] }[] = [];
  let tools: ToolStep[] = [];
  for (const m of messages) {
    if (m.role === "tool") {
      try {
        const t = JSON.parse(m.content) as ToolTrace;
        tools.push({ id: String(m.id), name: t.name, arguments: t.arguments, result: t.result, is_error: t.is_error });
      } catch {
        /* trace illisible : ignorée */
      }
      continue;
    }
    out.push({ message: m, tools: m.role === "assistant" ? tools : [] });
    if (m.role === "assistant") tools = [];
  }
  return out;
}

export function Chat({ onOpenModels }: { onOpenModels: () => void }) {
  const { status, refresh } = useStore();
  const [sessions, setSessions] = useState<Session[]>([]);
  const [current, setCurrent] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [pending, setPending] = useState<Pending | null>(null);
  const [lastSpeed, setLastSpeed] = useState<number | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loadingModel, setLoadingModel] = useState(false);
  const [config, setConfig] = useState<Config | null>(null);
  const [approvals, setApprovals] = useState<ApprovalRequest[]>([]);
  const bottom = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.config().then(setConfig);
  }, []);

  const loadSessions = useCallback(async () => {
    const list = await api.sessions();
    setSessions(list);
    return list;
  }, []);

  useEffect(() => {
    loadSessions().then((l) => l[0] && setCurrent((c) => c ?? l[0].id));
  }, [loadSessions]);

  // Conversation créée à l'instant par un envoi : son historique arrivera à la fin
  // de la réponse (le charger maintenant afficherait le message en double).
  const freshSession = useRef<string | null>(null);
  useEffect(() => {
    if (current && current !== freshSession.current) api.messages(current).then(setMessages);
    else setMessages([]);
    freshSession.current = null;
  }, [current]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [messages, pending]);

  // Précharge le modèle actif pour une première réponse rapide.
  const active = status?.active_model;
  useEffect(() => {
    // En mode cloud, inutile d'occuper la carte graphique d'avance.
    if (!active || !config || config.routing.mode === "cloud") return;
    setLoadingModel(true);
    api
      .warmUp()
      .catch((e) => setError(String(e)))
      .finally(() => {
        setLoadingModel(false);
        refresh();
      });
  }, [JSON.stringify(active), config?.routing.mode]); // eslint-disable-line react-hooks/exhaustive-deps

  const newSession = async (fresh = false) => {
    const s = await api.createSession();
    await loadSessions();
    if (fresh) freshSession.current = s.id;
    setCurrent(s.id);
    return s.id;
  };

  const send = async () => {
    const text = input.trim();
    if (!text || pending) return;
    setError(null);
    setInput("");
    const sid = current ?? (await newSession(true));
    setPending({ user: text, reasoning: "", content: "", tools: [] });
    try {
      const reply = await api.sendMessage(sid, text, (e) => {
        if (e.type === "approval") {
          const { type: _, ...req } = e;
          setApprovals((a) => [...a, req]);
        }
        setPending((p) => p && applyEvent(p, e));
      });
      setLastSpeed(reply.tokens_per_sec);
    } catch (e) {
      setError(String(e));
    } finally {
      setApprovals([]);
      setPending(null);
      setMessages(await api.messages(sid));
      loadSessions();
    }
  };

  const rename = async (s: Session) => {
    const title = window.prompt("Nouveau titre", s.title);
    if (title?.trim()) {
      await api.renameSession(s.id, title.trim());
      loadSessions();
    }
  };

  const remove = async (s: Session) => {
    if (!window.confirm(`Supprimer la conversation « ${s.title} » ?`)) return;
    await api.deleteSession(s.id);
    const list = await loadSessions();
    if (current === s.id) setCurrent(list[0]?.id ?? null);
  };

  const noModel = status && !status.active_model && !config?.routing.cloud;
  const session = sessions.find((s) => s.id === current);
  const setRoute = async (mode: RoutingMode | null) => {
    if (!current) return;
    await api.setSessionRoute(current, mode);
    loadSessions();
  };

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex w-64 shrink-0 flex-col border-r border-[var(--border)] bg-[var(--panel)]">
        <div className="p-3">
          <Button className="w-full" onClick={() => newSession()}>
            <MessageSquarePlus size={16} /> Nouvelle conversation
          </Button>
        </div>
        <nav className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-3">
          {sessions.map((s) => (
            <div
              key={s.id}
              className={`group flex items-center rounded-lg ${
                s.id === current ? "bg-[var(--panel-2)]" : "hover:bg-[var(--panel-2)]/60"
              }`}
            >
              <button className="min-w-0 flex-1 truncate px-3 py-2 text-left" onClick={() => setCurrent(s.id)} title={s.title}>
                {s.title}
              </button>
              <div className="hidden pr-1 group-hover:flex">
                <button className="rounded p-1 text-[var(--muted)] hover:text-[var(--text)]" onClick={() => rename(s)} aria-label="Renommer">
                  <Pencil size={13} />
                </button>
                <button className="rounded p-1 text-[var(--muted)] hover:text-[var(--danger)]" onClick={() => remove(s)} aria-label="Supprimer">
                  <Trash2 size={13} />
                </button>
              </div>
            </div>
          ))}
        </nav>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center justify-between gap-3 border-b border-[var(--border)] px-5 py-3">
          <button className="flex items-center gap-2 text-sm hover:text-[var(--accent)]" onClick={onOpenModels}>
            <span
              className={`size-2 rounded-full ${
                loadingModel ? "animate-pulse bg-amber-500" : status?.loaded_model || active?.source === "ollama" ? "bg-[var(--ok)]" : "bg-[var(--muted)]"
              }`}
            />
            <span className="font-medium">
              {(session?.route ?? config?.routing.mode) === "cloud" && config?.routing.cloud
                ? `${config.routing.cloud.model} (cloud)`
                : activeLabel(active ?? null, status?.installed_models ?? [])}
            </span>
            {loadingModel && <span className="text-[var(--muted)]">chargement en mémoire…</span>}
          </button>
          <div className="flex items-center gap-3">
            {lastSpeed != null && <span className="text-xs text-[var(--muted)]">{lastSpeed.toFixed(0)} jetons/s</span>}
            {session && config && (
              <select
                className="rounded-lg border border-[var(--border)] bg-[var(--panel)] px-2 py-1 text-sm"
                title="Où cette conversation est traitée"
                value={session.route ?? ""}
                onChange={(e) => setRoute((e.target.value || null) as RoutingMode | null)}
              >
                <option value="">Par défaut ({routingLabels[config.routing.mode].toLowerCase()})</option>
                {(Object.keys(routingLabels) as RoutingMode[]).map((m) => (
                  <option key={m} value={m}>
                    {routingLabels[m]}
                  </option>
                ))}
              </select>
            )}
          </div>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto max-w-3xl space-y-5 px-5 py-6">
            {messages.length === 0 && !pending && (
              <div className="py-20 text-center text-[var(--muted)]">
                {noModel ? (
                  <>
                    <p>Aucun modèle n'est sélectionné.</p>
                    <Button className="mt-3" variant="primary" onClick={onOpenModels}>
                      Choisir un modèle
                    </Button>
                  </>
                ) : (
                  <p className="text-lg">Que puis-je faire pour toi ?</p>
                )}
              </div>
            )}
            {groupHistory(messages).map(({ message: m, tools }) => (
              <div key={m.id} className="space-y-2">
                {tools.map((t) => (
                  <ToolCard key={t.id} step={t} />
                ))}
                <Bubble role={m.role} content={m.content} reasoning={m.reasoning ?? undefined} meta={m} />
              </div>
            ))}
            {pending && (
              <>
                <Bubble role="user" content={pending.user} />
                {pending.route && <RouteBadge route={pending.route} />}
                {pending.tools.map((t) => (
                  <ToolCard key={t.id} step={t} />
                ))}
                <Bubble role="assistant" content={pending.content} reasoning={pending.reasoning} streaming />
              </>
            )}
            <ErrorNote message={error} />
            <div ref={bottom} />
            {approvals[0] && (
              <ApprovalDialog
                request={approvals[0]}
                onAnswer={(a) => {
                  const req = approvals[0];
                  setApprovals((list) => list.slice(1));
                  api.answerApproval(req.id, a).catch((e) => setError(String(e)));
                }}
              />
            )}
          </div>
        </div>

        <footer className="border-t border-[var(--border)] px-5 py-4">
          <div className="mx-auto flex max-w-3xl items-end gap-2 rounded-xl border border-[var(--border)] bg-[var(--panel)] p-2 focus-within:border-[var(--accent)]">
            <textarea
              className="max-h-48 min-h-10 flex-1 resize-none bg-transparent px-2 py-2 outline-none"
              rows={1}
              placeholder={noModel ? "Choisis d'abord un modèle…" : "Écris à Claudio… (Entrée pour envoyer, Maj+Entrée pour aller à la ligne)"}
              value={input}
              disabled={!!noModel}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send();
                }
              }}
            />
            {pending ? (
              <Button variant="secondary" onClick={() => current && api.cancelChat(current)} aria-label="Arrêter">
                <Square size={16} />
              </Button>
            ) : (
              <Button variant="primary" onClick={send} disabled={!input.trim() || !!noModel} aria-label="Envoyer">
                <Send size={16} />
              </Button>
            )}
          </div>
        </footer>
      </section>
    </div>
  );
}

function Bubble({
  role,
  content,
  reasoning,
  streaming,
  meta,
}: {
  role: Message["role"];
  content: string;
  reasoning?: string;
  streaming?: boolean;
  meta?: Message;
}) {
  if (role === "user") {
    return (
      <div className="flex justify-end">
        <div className="max-w-[80%] rounded-2xl rounded-br-md bg-[var(--accent)] px-4 py-2.5 whitespace-pre-wrap text-[var(--accent-text)]">
          {content}
        </div>
      </div>
    );
  }
  return (
    <div className="space-y-2">
      {reasoning && <Reasoning text={reasoning} open={streaming && !content} />}
      {content ? (
        <div className="prose-claudio">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
        </div>
      ) : (
        streaming && !reasoning && <Spinner className="text-[var(--muted)]" />
      )}
      {meta?.duration_ms != null && (
        <p className="text-xs text-[var(--muted)]" title={meta.route ?? undefined}>
          {meta.model} · {meta.tokens ?? "?"} jetons · {(meta.duration_ms / 1000).toFixed(1).replace(".", ",")} s
          {meta.cost_usd != null && ` · ${formatUsd(meta.cost_usd)}`}
          {meta.route && meta.route !== "modèle local" && ` · ${meta.route}`}
        </p>
      )}
    </div>
  );
}

function RouteBadge({ route }: { route: Route }) {
  const cloud = route.target === "cloud";
  return (
    <p className="flex items-center gap-1.5 text-xs text-[var(--muted)]">
      {cloud ? <Cloud size={13} /> : <Cpu size={13} />}
      {cloud ? "Cloud" : "Local"} · {route.model} · {route.reason}
    </p>
  );
}

function Reasoning({ text, open }: { text: string; open?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const show = expanded || open;
  return (
    <div className="rounded-lg border border-[var(--border)] bg-[var(--panel)] text-sm">
      <button className="flex w-full items-center gap-2 px-3 py-2 text-[var(--muted)]" onClick={() => setExpanded(!expanded)}>
        <ChevronRight size={14} className={show ? "rotate-90 transition-transform" : "transition-transform"} />
        <Brain size={14} /> {open ? "Réflexion en cours…" : "Réflexion"}
      </button>
      {show && <div className="max-h-64 overflow-y-auto px-3 pb-3 whitespace-pre-wrap text-[var(--muted)]">{text}</div>}
    </div>
  );
}
