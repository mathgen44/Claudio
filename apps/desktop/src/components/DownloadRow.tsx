import { X } from "lucide-react";
import type { DownloadState } from "../store";
import { useStore } from "../store";
import { Button, ProgressBar, formatBytes } from "../ui";

const stageLabel: Record<string, string> = {
  resolving: "Recherche de la dernière version…",
  extracting: "Installation…",
};

/** Progression d'un téléchargement, avec annulation ou message d'erreur. */
export function DownloadRow({ id, state }: { id: string; state: DownloadState }) {
  const { cancel, dismiss } = useStore();
  const p = state.progress;
  const text =
    state.error ??
    (state.stage && stageLabel[state.stage]) ??
    (p
      ? `${formatBytes(p.downloaded)} / ${formatBytes(p.total)}${p.bytes_per_sec ? ` · ${formatBytes(p.bytes_per_sec)}/s` : ""}`
      : "Connexion…");
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between gap-3 text-sm">
        <span className="font-medium">{state.label}</span>
        <span className={state.error ? "text-[var(--danger)]" : "text-[var(--muted)]"}>{text}</span>
        {state.error ? (
          <Button variant="ghost" className="px-2 py-1" onClick={() => dismiss(id)} aria-label="Fermer">
            <X size={14} />
          </Button>
        ) : (
          <Button variant="ghost" className="px-2 py-1" onClick={() => cancel(id)}>
            Annuler
          </Button>
        )}
      </div>
      {!state.error && <ProgressBar value={p?.downloaded ?? 0} total={p?.total ?? null} />}
    </div>
  );
}
