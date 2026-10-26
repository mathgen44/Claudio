// Conversations : liste des sessions, fil de messages en continu, saisie.

import { Brain, ChevronRight, MessageSquarePlus, Pencil, Send, Square, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { activeLabel, api, type Message, type Session } from "../api";
import { useStore } from "../store";
import { Button, ErrorNote, Spinner } from "../ui";

interface Pending {
  user: string;
  reasoning: string;
  content: string;
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
  const bottom = useRef<HTMLDivElement>(null);

  const loadSessions = useCallback(async () => {
    const list = await api.sessions();
    setSessions(list);
    return list;
  }, []);

  useEffect(() => {
    loadSessions().then((l) => l[0] && setCurrent((c) => c ?? l[0].id));
  }, [loadSessions]);

  useEffect(() => {
    if (current) api.messages(current).then(setMessages);
    else setMessages([]);
  }, [current]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [messages, pending]);

  // Précharge le modèle actif pour une première réponse rapide.
  const active = status?.active_model;
  useEffect(() => {
    if (!active) return;
    setLoadingModel(true);
    api
      .warmUp()
      .catch((e) => setError(String(e)))
      .finally(() => {
        setLoadingModel(false);
        refresh();
      });
  }, [JSON.stringify(active)]); // eslint-disable-line react-hooks/exhaustive-deps

  const newSession = async () => {
    const s = await api.createSession();
    await loadSessions();
    setCurrent(s.id);
    return s.id;
  };

  const send = async () => {
    const text = input.trim();
    if (!text || pending) return;
    setError(null);
    setInput("");
    const sid = current ?? (await newSession());
    setPending({ user: text, reasoning: "", content: "" });
    try {
      const reply = await api.sendMessage(sid, text, (e) =>
        setPending((p) =>
          p && (e.type === "token" ? { ...p, content: p.content + e.text } : { ...p, reasoning: p.reasoning + e.text }),
        ),
      );
      setLastSpeed(reply.tokens_per_sec);
    } catch (e) {
      setError(String(e));
    } finally {
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

  const noModel = status && !status.active_model;

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex w-64 shrink-0 flex-col border-r border-[var(--border)] bg-[var(--panel)]">
        <div className="p-3">
          <Button className="w-full" onClick={newSession}>
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
            <span className="font-medium">{activeLabel(active ?? null, status?.installed_models ?? [])}</span>
            {loadingModel && <span className="text-[var(--muted)]">chargement en mémoire…</span>}
          </button>
          {lastSpeed != null && <span className="text-xs text-[var(--muted)]">{lastSpeed.toFixed(0)} jetons/s</span>}
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
            {messages.map((m) => (
              <Bubble key={m.id} role={m.role} content={m.content} meta={m} />
            ))}
            {pending && (
              <>
                <Bubble role="user" content={pending.user} />
                <Bubble role="assistant" content={pending.content} reasoning={pending.reasoning} streaming />
              </>
            )}
            <ErrorNote message={error} />
            <div ref={bottom} />
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
        <p className="text-xs text-[var(--muted)]">
          {meta.model} · {meta.tokens ?? "?"} jetons · {(meta.duration_ms / 1000).toFixed(1).replace(".", ",")} s
        </p>
      )}
    </div>
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
