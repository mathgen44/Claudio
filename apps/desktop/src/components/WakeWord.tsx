// Écoute du mot d'éveil, active sur tous les écrans quand elle est activée.

import { Ear } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../api";
import { VOICE_SETTINGS_EVENT, WakeListener, onVoiceActivity, pushVoiceCommand } from "../voice";

/** Petit signal sonore : « je vous écoute ». */
function chime() {
  const ctx = new AudioContext();
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.frequency.setValueAtTime(660, ctx.currentTime);
  osc.frequency.setValueAtTime(880, ctx.currentTime + 0.09);
  gain.gain.setValueAtTime(0.08, ctx.currentTime);
  gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.25);
  osc.connect(gain).connect(ctx.destination);
  osc.start();
  osc.stop(ctx.currentTime + 0.25);
  osc.onended = () => ctx.close();
}

export function WakeWord({ onWake }: { onWake: () => void }) {
  const [enabled, setEnabled] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const load = () =>
      Promise.all([api.config(), api.voiceStatus()]).then(([c, v]) => setEnabled(c.voice.wake_word && v.wake_missing.length === 0));
    load();
    window.addEventListener(VOICE_SETTINGS_EVENT, load);
    return () => window.removeEventListener(VOICE_SETTINGS_EVENT, load);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    setError(null);
    const listener = new WakeListener(
      (request) => {
        if (!request) chime();
        pushVoiceCommand(request);
        onWake();
      },
      (e) => setError(e),
    );
    const off = onVoiceActivity((busy) => listener.pause(busy));
    listener.start().catch((e) => setError(String(e).includes("NotAllowed") ? "Accès au micro refusé." : String(e)));
    return () => {
      off();
      listener.stop();
    };
  }, [enabled]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!enabled) return null;
  return (
    <div
      className="flex size-10 items-center justify-center text-[var(--muted)]"
      title={error ? `Mot d'éveil : ${error}` : "À l'écoute du mot d'éveil"}
      aria-label="À l'écoute du mot d'éveil"
    >
      <Ear size={18} className={error ? "text-[var(--danger)]" : "animate-pulse text-[var(--accent)]"} />
    </div>
  );
}
