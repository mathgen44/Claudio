// Carte d'un appel d'outil dans une conversation (repliable).

import { AlertTriangle, Check, ChevronRight, Wrench } from "lucide-react";
import { useState } from "react";
import { toolLabel } from "../api";
import { Spinner } from "../ui";

export interface ToolStep {
  id: string;
  name: string;
  arguments: string;
  result?: string;
  is_error?: boolean;
}

/** Résumé d'une ligne des arguments (chemin, adresse, commande…). */
function summary(args: string): string {
  try {
    const v = JSON.parse(args) as Record<string, unknown>;
    const main = v.path ?? v.url ?? v.query ?? v.command ?? v.source ?? Object.values(v)[0];
    return typeof main === "string" ? main : JSON.stringify(v);
  } catch {
    return args;
  }
}

export function ToolCard({ step }: { step: ToolStep }) {
  const [open, setOpen] = useState(false);
  const running = step.result == null;
  return (
    <div className="rounded-lg border border-[var(--border)] bg-[var(--panel)] text-sm">
      <button className="flex w-full items-center gap-2 px-3 py-2 text-left" onClick={() => setOpen(!open)}>
        <ChevronRight size={14} className={`shrink-0 text-[var(--muted)] transition-transform ${open ? "rotate-90" : ""}`} />
        {running ? (
          <Spinner className="size-3.5 text-[var(--accent)]" />
        ) : step.is_error ? (
          <AlertTriangle size={14} className="shrink-0 text-[var(--danger)]" />
        ) : (
          <Check size={14} className="shrink-0 text-[var(--ok)]" />
        )}
        <Wrench size={13} className="shrink-0 text-[var(--muted)]" />
        <span className="shrink-0 font-medium">{toolLabel(step.name)}</span>
        <span className="min-w-0 truncate text-[var(--muted)]">{summary(step.arguments)}</span>
      </button>
      {open && (
        <div className="space-y-2 border-t border-[var(--border)] px-3 py-2">
          <pre className="overflow-x-auto font-mono text-xs whitespace-pre-wrap text-[var(--muted)]">{step.arguments}</pre>
          {step.result != null && (
            <pre
              className={`max-h-64 overflow-auto rounded bg-[var(--panel-2)] p-2 font-mono text-xs whitespace-pre-wrap ${
                step.is_error ? "text-[var(--danger)]" : ""
              }`}
            >
              {step.result}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}
