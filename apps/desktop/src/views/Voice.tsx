// Voix : dictée, mot d'éveil et lecture des réponses, tout en local.

import { Download, Play, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type Config, type SystemVoice, type VoiceComponent, type VoiceSettings, type VoiceStatus } from "../api";
import { VoiceSetup } from "../components/VoiceSetup";
import { Badge, Button, Card, ErrorNote, PageHeader, ProgressBar, formatBytes } from "../ui";
import { VOICE_SETTINGS_EVENT, speak } from "../voice";
import { TelegramCard } from "./Telegram";

const LANGUAGES: [string, string][] = [
  ["fr", "Français"],
  ["en", "Anglais"],
  ["de", "Allemand"],
  ["es", "Espagnol"],
  ["it", "Italien"],
];

export function Voice() {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [status, setStatus] = useState<VoiceStatus | null>(null);
  const [voices, setVoices] = useState<SystemVoice[]>([]);
  const [phrase, setPhrase] = useState("");
  const [setup, setSetup] = useState<{ missing: string[]; purpose: "dictation" | "wake" } | null>(null);
  const [installing, setInstalling] = useState<Record<string, number>>({});
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const reload = () => api.voiceStatus().then(setStatus);
  useEffect(() => {
    api.config().then((c) => {
      setCfg(c);
      setPhrase(c.voice.wake_phrase);
    });
    reload();
    api.systemVoices().then(setVoices).catch(() => setVoices([]));
  }, []);

  if (!cfg || !status) return null;
  const v = cfg.voice;

  const save = async (patch: Partial<VoiceSettings>) => {
    const next = { ...v, ...patch };
    setError(null);
    try {
      await api.setVoiceSettings(next);
      setCfg({ ...cfg, voice: next });
      window.dispatchEvent(new Event(VOICE_SETTINGS_EVENT));
      setSaved("Enregistré");
      setTimeout(() => setSaved(null), 2000);
      await reload();
    } catch (e) {
      setError(String(e));
    }
  };

  const install = async (c: VoiceComponent) => {
    setInstalling((m) => ({ ...m, [c.id]: 0 }));
    try {
      await api.installVoice(c.id, (p) => setInstalling((m) => ({ ...m, [c.id]: p.downloaded })));
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(({ [c.id]: _, ...rest }) => rest);
      await reload();
    }
  };

  const remove = async (c: VoiceComponent) => {
    if (!window.confirm(`Supprimer « ${c.label} » ?`)) return;
    await api.removeVoice(c.id).catch((e) => setError(String(e)));
    await reload();
  };

  const toggleWake = async (on: boolean) => {
    if (on && status.wake_missing.length) {
      setSetup({ missing: status.wake_missing, purpose: "wake" });
      return;
    }
    await save({ wake_word: on });
  };

  const byKind = (k: VoiceComponent["kind"]) => status.components.filter((c) => c.kind === k);
  const sttModels = byKind("stt");
  const auto = sttModels.find((c) => c.id === status.stt_model);
  const select = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";
  const piper = status.components.find((c) => c.kind === "ttsruntime");
  const piperReady = piper?.installed && byKind("voice").some((c) => c.id === v.piper_voice && c.installed);

  const row = (c: VoiceComponent) => (
    <li key={c.id} className="space-y-1.5 py-2">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 font-medium">
            {c.label} {c.installed && <Badge tone="ok">installé</Badge>}
          </div>
          <div className="text-sm text-[var(--muted)]">
            {c.description} · {formatBytes(c.size)}
          </div>
        </div>
        {c.installed ? (
          <Button variant="ghost" onClick={() => remove(c)} aria-label={`Supprimer ${c.label}`}>
            <Trash2 size={15} />
          </Button>
        ) : (
          <Button onClick={() => install(c)} disabled={c.id in installing}>
            <Download size={15} /> Installer
          </Button>
        )}
      </div>
      {c.id in installing && <ProgressBar value={installing[c.id]} total={c.size} />}
    </li>
  );

  return (
    <div className="mx-auto max-w-3xl space-y-6 px-6 py-8">
      <PageHeader
        title="Voix et Telegram"
        subtitle={saved ?? "La reconnaissance et la synthèse vocales fonctionnent sur cet ordinateur : votre voix n'en sort pas."}
      />
      <ErrorNote message={error} />

      <Card className="space-y-4">
        <h2 className="font-medium">Dictée</h2>
        <p className="text-sm text-[var(--muted)]">Le bouton micro de la conversation enregistre jusqu'au silence, puis envoie la transcription.</p>
        <div className="flex flex-wrap gap-3">
          <label className="space-y-1 text-sm">
            <div className="font-medium">Modèle</div>
            <select className={select} value={v.stt_model ?? ""} onChange={(e) => save({ stt_model: e.target.value || null })}>
              <option value="">Conseillé pour cette machine{auto && !v.stt_model ? ` (${auto.label})` : ""}</option>
              {sttModels.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.label}
                  {c.installed ? "" : " (à installer)"}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1 text-sm">
            <div className="font-medium">Langue parlée</div>
            <select className={select} value={v.language} onChange={(e) => save({ language: e.target.value })}>
              {LANGUAGES.map(([code, name]) => (
                <option key={code} value={code}>
                  {name}
                </option>
              ))}
            </select>
          </label>
        </div>
        {status.dictation_missing.length > 0 && (
          <Button variant="primary" onClick={() => setSetup({ missing: status.dictation_missing, purpose: "dictation" })}>
            <Download size={15} /> Installer la dictée
          </Button>
        )}
        <ul className="divide-y divide-[var(--border)]">{[...byKind("sttruntime"), ...sttModels].map(row)}</ul>
      </Card>

      <Card className="space-y-4">
        <h2 className="font-medium">Mot d'éveil</h2>
        <label className="flex items-start gap-2">
          <input type="checkbox" className="mt-1 accent-[var(--accent)]" checked={v.wake_word} onChange={(e) => toggleWake(e.target.checked)} />
          <span>
            Écouter en permanence « {v.wake_phrase} »
            <span className="block text-sm text-[var(--muted)]">
              Le micro reste ouvert tant que Claudio tourne ; seuls les courts passages de parole sont analysés, ici, par le petit modèle
              Whisper Base. Dites « {v.wake_phrase}, … » suivi de votre demande, ou le mot seul puis votre demande après le signal.
            </span>
          </span>
        </label>
        <div className="flex gap-2">
          <input
            className="flex-1 rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2"
            value={phrase}
            onChange={(e) => setPhrase(e.target.value)}
            aria-label="Mot d'éveil"
          />
          <Button disabled={!phrase.trim() || phrase.trim() === v.wake_phrase} onClick={() => save({ wake_phrase: phrase.trim() })}>
            Enregistrer
          </Button>
        </div>
      </Card>

      <Card className="space-y-4">
        <h2 className="font-medium">Lecture des réponses</h2>
        <div className="flex flex-wrap gap-3">
          <label className="space-y-1 text-sm">
            <div className="font-medium">Lire à voix haute</div>
            <select className={select} value={v.speak} onChange={(e) => save({ speak: e.target.value as VoiceSettings["speak"] })}>
              <option value="voice">Quand je parle à Claudio</option>
              <option value="always">Toujours</option>
              <option value="never">Jamais</option>
            </select>
          </label>
          <label className="space-y-1 text-sm">
            <div className="font-medium">Moteur</div>
            <select className={select} value={v.tts_engine} onChange={(e) => save({ tts_engine: e.target.value as VoiceSettings["tts_engine"] })}>
              <option value="system">Voix Windows</option>
              <option value="piper">Piper (plus naturel)</option>
            </select>
          </label>
          {v.tts_engine === "system" ? (
            <label className="space-y-1 text-sm">
              <div className="font-medium">Voix</div>
              <select className={select} value={v.system_voice ?? ""} onChange={(e) => save({ system_voice: e.target.value || null })}>
                <option value="">Automatique (langue choisie)</option>
                {voices.map((sv) => (
                  <option key={sv.name} value={sv.name}>
                    {sv.name.replace(/^Microsoft /, "").replace(/ Desktop$/, "")} ({sv.culture})
                  </option>
                ))}
              </select>
            </label>
          ) : (
            <label className="space-y-1 text-sm">
              <div className="font-medium">Voix</div>
              <select className={select} value={v.piper_voice} onChange={(e) => save({ piper_voice: e.target.value })}>
                {byKind("voice").map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label}
                    {c.installed ? "" : " (à installer)"}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="space-y-1 text-sm">
            <div className="font-medium">Débit</div>
            <input
              type="range"
              min={-5}
              max={5}
              value={v.rate}
              onChange={(e) => save({ rate: Number(e.target.value) })}
              className="accent-[var(--accent)]"
              aria-label="Débit de la voix"
            />
          </label>
        </div>
        {v.tts_engine === "piper" && !piperReady && (
          <p className="text-sm text-amber-600">Piper ou la voix choisie n'est pas installé : la voix Windows est utilisée en attendant.</p>
        )}
        <Button onClick={() => speak("Bonjour, je suis Claudio. Voici à quoi ressemble ma voix.").catch((e) => setError(String(e)))}>
          <Play size={15} /> Essayer
        </Button>
        {v.tts_engine === "piper" && <ul className="divide-y divide-[var(--border)]">{[...byKind("ttsruntime"), ...byKind("voice")].map(row)}</ul>}
      </Card>

      <TelegramCard />

      {setup && (
        <VoiceSetup
          status={status}
          missing={setup.missing}
          purpose={setup.purpose}
          onClose={() => setSetup(null)}
          onDone={async () => {
            const wasWake = setup.purpose === "wake";
            setSetup(null);
            await reload();
            if (wasWake) await save({ wake_word: true });
          }}
        />
      )}
    </div>
  );
}
