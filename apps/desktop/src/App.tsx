import clsx from "clsx";
import { Boxes, Cloud as CloudIcon, MessageSquare, Settings as SettingsIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { DownloadRow } from "./components/DownloadRow";
import { StoreProvider, useStore } from "./store";
import { ErrorNote, Spinner } from "./ui";
import { Chat } from "./views/Chat";
import { Cloud } from "./views/Cloud";
import { Models } from "./views/Models";
import { Onboarding } from "./views/Onboarding";
import { Settings } from "./views/Settings";

type View = "chat" | "models" | "cloud" | "settings" | "onboarding";

export default function App() {
  return (
    <StoreProvider>
      <Shell />
    </StoreProvider>
  );
}

function Shell() {
  const { status, statusError, downloads } = useStore();
  const [view, setView] = useState<View | null>(null);

  useEffect(() => {
    if (status && view == null) setView(status.onboarding_done ? "chat" : "onboarding");
  }, [status, view]);

  if (!status || view == null) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6">
        {statusError ? <ErrorNote message={statusError} /> : <Spinner className="size-6 text-[var(--accent)]" />}
        {!statusError && <p className="text-[var(--muted)]">Analyse de la machine…</p>}
      </div>
    );
  }

  // Téléchargements en cours, visibles depuis tous les écrans sauf ceux qui les affichent déjà.
  const background = view === "chat" ? Object.entries(downloads) : [];

  return (
    <div className="flex h-full">
      <nav className="flex w-16 shrink-0 flex-col items-center gap-1 border-r border-[var(--border)] bg-[var(--panel)] py-3">
        <div className="mb-3 flex size-9 items-center justify-center rounded-xl bg-[var(--accent)] font-semibold text-[var(--accent-text)]">
          C
        </div>
        <NavButton label="Conversations" active={view === "chat"} onClick={() => setView("chat")}>
          <MessageSquare size={20} />
        </NavButton>
        <NavButton label="Modèles" active={view === "models"} onClick={() => setView("models")}>
          <Boxes size={20} />
        </NavButton>
        <NavButton label="Cloud et budgets" active={view === "cloud"} onClick={() => setView("cloud")}>
          <CloudIcon size={20} />
        </NavButton>
        <div className="flex-1" />
        <NavButton label="Paramètres" active={view === "settings" || view === "onboarding"} onClick={() => setView("settings")}>
          <SettingsIcon size={20} />
        </NavButton>
      </nav>
      <main className="flex min-w-0 flex-1 flex-col">
        {background.length > 0 && (
          <div className="space-y-2 border-b border-[var(--border)] bg-[var(--panel)] px-5 py-2">
            {background.map(([id, d]) => (
              <DownloadRow key={id} id={id} state={d} />
            ))}
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto">
          {view === "onboarding" && <Onboarding onDone={() => setView("chat")} />}
          {view === "chat" && <Chat onOpenModels={() => setView("models")} />}
          {view === "models" && <Models />}
          {view === "cloud" && <Cloud />}
          {view === "settings" && <Settings onRestartOnboarding={() => setView("onboarding")} />}
        </div>
      </main>
    </div>
  );
}

function NavButton({ label, active, onClick, children }: { label: string; active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      title={label}
      aria-label={label}
      onClick={onClick}
      className={clsx(
        "flex size-10 items-center justify-center rounded-lg transition-colors",
        active ? "bg-[var(--accent-soft)] text-[var(--accent)]" : "text-[var(--muted)] hover:bg-[var(--panel-2)] hover:text-[var(--text)]",
      )}
    >
      {children}
    </button>
  );
}
