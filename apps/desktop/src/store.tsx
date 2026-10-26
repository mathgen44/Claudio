// État partagé : statut de la machine et téléchargements en cours, qui doivent
// survivre aux changements d'écran.

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { api, modelKey, type InstallEvent, type Progress, type Status } from "./api";

export interface DownloadState {
  label: string;
  progress: Progress | null;
  stage?: InstallEvent["stage"];
  error?: string;
}

interface Store {
  status: Status | null;
  statusError: string | null;
  refresh: () => Promise<void>;
  downloads: Record<string, DownloadState>;
  downloadModel: (modelId: string, quant: string, label: string) => Promise<boolean>;
  installRuntime: () => Promise<boolean>;
  cancel: (key: string) => void;
  dismiss: (key: string) => void;
}

const Ctx = createContext<Store | null>(null);

export const RUNTIME_KEY = "runtime";

export function StoreProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [downloads, setDownloads] = useState<Record<string, DownloadState>>({});

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.status());
      setStatusError(null);
    } catch (e) {
      setStatusError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const patch = (key: string, p: Partial<DownloadState>) =>
    setDownloads((d) => ({ ...d, [key]: { ...d[key], ...p } as DownloadState }));

  const run = async (key: string, label: string, job: () => Promise<unknown>) => {
    setDownloads((d) => ({ ...d, [key]: { label, progress: null } }));
    try {
      await job();
      setDownloads(({ [key]: _, ...rest }) => rest);
      await refresh();
      return true;
    } catch (e) {
      patch(key, { error: String(e) });
      return false;
    }
  };

  const downloadModel = (modelId: string, quant: string, label: string) =>
    run(modelKey(modelId, quant), label, () =>
      api.installModel(modelId, quant, (progress) => patch(modelKey(modelId, quant), { progress })),
    );

  const installRuntime = () =>
    run(RUNTIME_KEY, "Moteur llama.cpp", () =>
      api.installRuntime((e) =>
        patch(RUNTIME_KEY, e.stage === "downloading" ? { stage: e.stage, progress: e.progress } : { stage: e.stage }),
      ),
    );

  const cancel = (key: string) => api.cancelDownload(key);
  const dismiss = (key: string) => setDownloads(({ [key]: _, ...rest }) => rest);

  return (
    <Ctx.Provider value={{ status, statusError, refresh, downloads, downloadModel, installRuntime, cancel, dismiss }}>
      {children}
    </Ctx.Provider>
  );
}

export function useStore(): Store {
  const s = useContext(Ctx);
  if (!s) throw new Error("StoreProvider manquant");
  return s;
}
