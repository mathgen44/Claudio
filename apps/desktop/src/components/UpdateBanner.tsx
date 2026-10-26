// Bandeau « nouvelle version » : vérification au démarrage si elle est activée.

import { Download, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../api";
import { Button, ProgressBar } from "../ui";
import { findUpdate, installUpdate, type Update } from "../updates";

export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number | null } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .config()
      .then((c) => (c.check_updates ? findUpdate() : null))
      .then(setUpdate)
      // Hors ligne ou dépôt inaccessible : on réessaiera au prochain démarrage.
      .catch(() => {});
  }, []);

  if (!update) return null;
  const install = () => {
    setError(null);
    setProgress({ done: 0, total: null });
    installUpdate(update, (done, total) => setProgress({ done, total })).catch((e) => {
      setProgress(null);
      setError(String(e));
    });
  };

  return (
    <div className="flex items-center gap-3 border-b border-[var(--border)] bg-[var(--accent-soft)] px-5 py-2 text-sm" role="status">
      <span className="flex-1">
        {error ? `Mise à jour impossible : ${error}` : `Claudio ${update.version} est disponible (vous avez la ${update.currentVersion}).`}
      </span>
      {progress ? (
        <div className="w-48">
          <ProgressBar value={progress.done} total={progress.total} />
        </div>
      ) : (
        <>
          <Button variant="primary" onClick={install}>
            <Download size={15} /> Installer et redémarrer
          </Button>
          <Button variant="ghost" onClick={() => setUpdate(null)} aria-label="Plus tard">
            <X size={15} />
          </Button>
        </>
      )}
    </div>
  );
}
