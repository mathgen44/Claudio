// Canal Telegram : jeton du robot, jumelage des conversations, état.

import { Send, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type TelegramStatus } from "../api";
import { Badge, Button, Card, ErrorNote } from "../ui";

export function TelegramCard() {
  const [st, setSt] = useState<TelegramStatus | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = () => api.telegramStatus().then(setSt);
  useEffect(() => {
    reload();
  }, []);
  // Pendant un jumelage, l'arrivée de la nouvelle conversation est guettée.
  useEffect(() => {
    if (!st?.pairing_code) return;
    const t = window.setInterval(reload, 2000);
    return () => window.clearInterval(t);
  }, [st?.pairing_code]);

  if (!st) return null;

  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      reload();
    }
  };

  return (
    <Card className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <h2 className="flex items-center gap-2 font-medium">
          <Send size={16} /> Telegram
        </h2>
        {st.running ? <Badge tone="ok">connecté{st.bot ? ` · ${st.bot}` : ""}</Badge> : st.has_token ? <Badge>arrêté</Badge> : null}
      </div>
      <p className="text-sm text-[var(--muted)]">
        Parlez à Claudio depuis votre téléphone. Les messages transitent par les serveurs de Telegram ; le travail, lui, est fait sur cet
        ordinateur, qui doit rester allumé avec Claudio ouvert (ou dans la zone de notification). Seules les conversations jumelées
        obtiennent une réponse, et chaque action sensible vous est demandée par des boutons.
      </p>
      <ErrorNote message={error ?? st.error} />

      <div className="space-y-1.5">
        <div className="font-medium">Jeton du robot</div>
        <div className="text-sm text-[var(--muted)]">
          Créez un robot avec @BotFather dans Telegram (commande /newbot), puis collez ici le jeton qu'il vous donne. Il est stocké chiffré
          dans le Gestionnaire d'identifiants Windows.
        </div>
        <div className="flex gap-2">
          <input
            type="password"
            className="flex-1 rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2"
            placeholder={st.has_token ? "Un jeton est enregistré" : "123456789:AA…"}
            value={token}
            onChange={(e) => setToken(e.target.value)}
            autoComplete="off"
          />
          <Button disabled={!token.trim() || busy} onClick={() => run(async () => (await api.setTelegramToken(token.trim()), setToken("")))}>
            Vérifier et enregistrer
          </Button>
          {st.has_token && (
            <Button variant="danger" disabled={busy} onClick={() => run(() => api.setTelegramToken(null))}>
              Supprimer
            </Button>
          )}
        </div>
      </div>

      {st.has_token && (
        <>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className="accent-[var(--accent)]"
              checked={st.enabled}
              onChange={(e) => run(() => api.setTelegramEnabled(e.target.checked))}
            />
            Activer le canal Telegram
          </label>

          <div className="space-y-2">
            <div className="flex items-center justify-between gap-3">
              <div className="font-medium">Conversations jumelées</div>
              <Button disabled={!st.enabled || busy} onClick={() => run(api.telegramPairingCode)}>
                Jumeler un téléphone
              </Button>
            </div>
            {st.pairing_code && (
              <p className="rounded-lg bg-[var(--panel-2)] px-3 py-2 text-sm">
                Dans Telegram, ouvrez {st.bot ?? "votre robot"} et envoyez :{" "}
                <code className="font-mono font-semibold select-all">/jumeler {st.pairing_code}</code> (code valable 10 minutes).
              </p>
            )}
            {st.chats.length === 0 ? (
              <p className="text-sm text-[var(--muted)]">Aucune pour l'instant.</p>
            ) : (
              <ul className="divide-y divide-[var(--border)]">
                {st.chats.map((c) => (
                  <li key={c.id} className="flex items-center justify-between py-1.5">
                    <span>{c.name}</span>
                    <Button
                      variant="ghost"
                      aria-label={`Retirer ${c.name}`}
                      onClick={() => window.confirm(`Ne plus répondre à ${c.name} ?`) && run(() => api.removeTelegramChat(c.id))}
                    >
                      <Trash2 size={15} />
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </Card>
  );
}
