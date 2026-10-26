// Installation des éléments vocaux manquants (moteur et modèle Whisper), avec progression.

import { Mic } from "lucide-react";
import { useState } from "react";
import { api, type Progress, type VoiceStatus } from "../api";
import { Button, ErrorNote, ProgressBar, formatBytes } from "../ui";

export function VoiceSetup({
  status,
  missing,
  purpose,
  onDone,
  onClose,
}: {
  status: VoiceStatus;
  missing: string[];
  purpose: "dictation" | "wake";
  onDone: () => void;
  onClose: () => void;
}) {
  const items = status.components.filter((c) => missing.includes(c.id));
  const total = items.reduce((n, c) => n + c.size, 0);
  const [done, setDone] = useState(0);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const install = async () => {
    setRunning(true);
    setError(null);
    let base = 0;
    try {
      for (const c of items) {
        await api.installVoice(c.id, (p) => {
          setProgress(p);
          setDone(base + p.downloaded);
        });
        base += c.size;
      }
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" role="dialog" aria-modal="true" aria-labelledby="voice-setup-title">
      <div className="w-full max-w-md space-y-4 rounded-xl border border-[var(--border)] bg-[var(--panel)] p-5 shadow-2xl">
        <h2 id="voice-setup-title" className="flex items-center gap-2 text-lg font-semibold">
          <Mic size={18} /> {purpose === "wake" ? "Activer le mot d'éveil" : "Activer la dictée"}
        </h2>
        <p className="text-sm text-[var(--muted)]">
          La reconnaissance vocale fonctionne entièrement sur cet ordinateur : votre voix n'est envoyée nulle part. Il faut
          d'abord télécharger :
        </p>
        <ul className="space-y-1 text-sm">
          {items.map((c) => (
            <li key={c.id} className="flex justify-between gap-3">
              <span>{c.label}</span>
              <span className="text-[var(--muted)]">{formatBytes(c.size)}</span>
            </li>
          ))}
        </ul>
        {running && (
          <div className="space-y-1">
            <ProgressBar value={done} total={total} />
            <p className="text-right text-xs text-[var(--muted)]">
              {formatBytes(done)} / {formatBytes(total)}
              {progress?.bytes_per_sec ? ` · ${formatBytes(progress.bytes_per_sec)}/s` : ""}
            </p>
          </div>
        )}
        <ErrorNote message={error} />
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} disabled={running}>
            Plus tard
          </Button>
          <Button variant="primary" onClick={install} disabled={running}>
            Télécharger ({formatBytes(total)})
          </Button>
        </div>
      </div>
    </div>
  );
}
