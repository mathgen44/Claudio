// Fenêtre de demande d'autorisation : ce que l'assistant veut faire, exactement.

import { ShieldAlert, ShieldQuestion } from "lucide-react";
import { useEffect, useRef } from "react";
import { toolLabel, type Answer, type ApprovalRequest } from "../api";
import { Button } from "../ui";

const LABELS: Record<Exclude<Answer, "deny">, string> = {
  once: "Autoriser une fois",
  session: "Pour cette conversation",
  always: "Toujours",
};

export function ApprovalDialog({
  request,
  onAnswer,
  context,
}: {
  request: ApprovalRequest;
  onAnswer: (a: Answer) => void;
  /** Origine de la demande (tâche de fond). */
  context?: string;
}) {
  const denyRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Le refus a le focus : Entrée ne peut pas autoriser par mégarde.
    denyRef.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onAnswer("deny");
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [request.id, onAnswer]);

  const Icon = request.dangerous ? ShieldAlert : ShieldQuestion;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" role="dialog" aria-modal="true" aria-labelledby="approval-title">
      <div
        className={`w-full max-w-lg space-y-4 rounded-xl border bg-[var(--panel)] p-5 shadow-2xl ${
          request.dangerous ? "border-[var(--danger)]" : "border-[var(--border)]"
        }`}
      >
        <div className="flex items-start gap-3">
          <Icon size={22} className={request.dangerous ? "text-[var(--danger)]" : "text-[var(--accent)]"} />
          <div className="min-w-0">
            <h2 id="approval-title" className="font-semibold">
              {request.title}
            </h2>
            <p className="text-sm text-[var(--muted)]">
              {context ?? "Demandé par l'assistant"} · outil « {toolLabel(request.tool)} »
            </p>
          </div>
        </div>
        <pre className="max-h-64 overflow-auto rounded-lg bg-[var(--panel-2)] p-3 font-mono text-xs whitespace-pre-wrap break-all">
          {request.detail}
        </pre>
        <div className="flex flex-wrap justify-end gap-2">
          <Button ref={denyRef} variant="secondary" onClick={() => onAnswer("deny")}>
            Refuser
          </Button>
          {request.options
            .filter((o): o is Exclude<Answer, "deny"> => o !== "deny")
            .map((o, i) => (
              <Button
                key={o}
                variant={i === 0 ? (request.dangerous ? "danger" : "primary") : "secondary"}
                className={i === 0 && request.dangerous ? "border border-[var(--danger)]" : ""}
                onClick={() => onAnswer(o)}
              >
                {o === "session" && context ? "Pour cette tâche" : LABELS[o]}
              </Button>
            ))}
        </div>
      </div>
    </div>
  );
}
