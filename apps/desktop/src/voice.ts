// Audio côté interface : capture du micro (16 kHz mono), détection de la parole,
// écoute du mot d'éveil et lecture des réponses. La reconnaissance elle-même se
// fait dans le cœur Rust (Whisper, en local).

import { api } from "./api";

const RATE = 16000;

/** WAV PCM 16 bits mono. */
export function encodeWav(samples: Float32Array, rate = RATE): Uint8Array {
  const buf = new ArrayBuffer(44 + samples.length * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  v.setUint32(4, 36 + samples.length * 2, true);
  str(8, "WAVEfmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  str(36, "data");
  v.setUint32(40, samples.length * 2, true);
  samples.forEach((s, i) => v.setInt16(44 + i * 2, Math.max(-1, Math.min(1, s)) * 0x7fff, true));
  return new Uint8Array(buf);
}

function concat(chunks: Float32Array[]): Float32Array {
  const out = new Float32Array(chunks.reduce((n, c) => n + c.length, 0));
  let o = 0;
  for (const c of chunks) {
    out.set(c, o);
    o += c.length;
  }
  return out;
}

const rms = (c: Float32Array) => Math.sqrt(c.reduce((s, x) => s + x * x, 0) / c.length);

/** Micro ouvert : chaque bloc d'environ 128 ms est remis à `onChunk`. */
class Microphone {
  private ctx?: AudioContext;
  private stream?: MediaStream;
  private node?: ScriptProcessorNode;

  async open(onChunk: (c: Float32Array) => void) {
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
    });
    // Le navigateur rééchantillonne lui-même à 16 kHz.
    this.ctx = new AudioContext({ sampleRate: RATE });
    const src = this.ctx.createMediaStreamSource(this.stream);
    this.node = this.ctx.createScriptProcessor(2048, 1, 1);
    this.node.onaudioprocess = (e) => onChunk(new Float32Array(e.inputBuffer.getChannelData(0)));
    src.connect(this.node);
    this.node.connect(this.ctx.destination);
  }

  close() {
    this.node?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    this.ctx?.close();
  }
}

/** Seuil de parole, adapté au bruit de fond mesuré. */
class SpeechGate {
  private floor = 0.004;
  level = 0;

  push(c: Float32Array): boolean {
    this.level = rms(c);
    const speaking = this.level > Math.max(0.012, this.floor * 3);
    // Le bruit de fond n'est appris que pendant les silences.
    if (!speaking) this.floor = this.floor * 0.95 + this.level * 0.05;
    return speaking;
  }
}

export interface DictationOptions {
  /** Silence (ms) après la parole qui termine l'enregistrement. */
  silenceMs?: number;
  maxMs?: number;
  /** Niveau sonore (0 à 1) pour l'animation du bouton. */
  onLevel?: (level: number) => void;
}

/** Enregistrement d'une dictée, arrêté par un silence, la durée maximale ou `stop()`. */
export class Dictation {
  private mic = new Microphone();
  private chunks: Float32Array[] = [];
  private finish?: () => void;
  readonly done: Promise<Uint8Array | null>;

  constructor(private opts: DictationOptions = {}) {
    this.done = new Promise((resolve) => {
      let closed = false;
      this.finish = () => {
        if (closed) return;
        closed = true;
        this.mic.close();
        // Rien d'audible : pas de transcription.
        resolve(this.heard ? encodeWav(concat(this.chunks)) : null);
      };
    });
  }

  private heard = false;

  async start() {
    setActivity("dictation", true);
    this.done.finally(() => setActivity("dictation", false));
    const gate = new SpeechGate();
    const silenceMs = this.opts.silenceMs ?? 1400;
    const maxMs = this.opts.maxMs ?? 60000;
    let quiet = 0;
    let total = 0;
    let finished = false;
    await this.mic.open((c) => {
      if (finished) return;
      const ms = (c.length / RATE) * 1000;
      this.chunks.push(c);
      total += ms;
      const speaking = gate.push(c);
      this.opts.onLevel?.(Math.min(1, gate.level * 12));
      if (speaking) {
        this.heard = true;
        quiet = 0;
      } else quiet += ms;
      // Sans parole au bout de 6 s, on abandonne.
      if ((this.heard && quiet > silenceMs) || total > maxMs || (!this.heard && total > 6000)) {
        finished = true;
        this.finish?.();
      }
    });
  }

  stop() {
    this.finish?.();
  }
}

/**
 * Écoute continue du mot d'éveil : chaque passage de parole court est transcrit
 * par le petit modèle Whisper ; `onWake` reçoit la demande qui suit le mot.
 */
export class WakeListener {
  private mic = new Microphone();
  private paused = false;
  private busy = false;

  constructor(
    private onWake: (request: string) => void,
    private onError: (e: string) => void,
  ) {}

  async start() {
    const gate = new SpeechGate();
    // 0,4 s d'avance pour ne pas couper le début du mot.
    const preroll: Float32Array[] = [];
    let segment: Float32Array[] | null = null;
    let quiet = 0;
    let length = 0;
    await this.mic.open((c) => {
      if (this.paused || this.busy) return;
      const ms = (c.length / RATE) * 1000;
      const speaking = gate.push(c);
      if (!segment) {
        preroll.push(c);
        if (preroll.length > 3) preroll.shift();
        if (speaking) {
          segment = [...preroll];
          quiet = 0;
          length = 0;
        }
        return;
      }
      segment.push(c);
      length += ms;
      quiet = speaking ? 0 : quiet + ms;
      if (quiet > 700 || length > 7000) {
        const audio = segment;
        segment = null;
        preroll.length = 0;
        if (length >= 300 && length <= 7000) this.check(concat(audio));
      }
    });
  }

  private async check(samples: Float32Array) {
    this.busy = true;
    try {
      const t = await api.transcribe(encodeWav(samples), true);
      if (t.wake !== null) this.onWake(t.wake);
    } catch (e) {
      this.onError(String(e));
    } finally {
      this.busy = false;
    }
  }

  /** Suspend l'écoute (pendant une dictée ou la lecture d'une réponse). */
  pause(p: boolean) {
    this.paused = p;
  }

  stop() {
    this.mic.close();
  }
}

let current: HTMLAudioElement | null = null;

/** Lit une réponse à voix haute ; une nouvelle lecture interrompt la précédente. */
export async function speak(text: string): Promise<void> {
  const wav = await api.synthesize(text);
  stopSpeaking();
  if (!wav) return;
  const url = URL.createObjectURL(new Blob([wav], { type: "audio/wav" }));
  const audio = new Audio(url);
  current = audio;
  speakingListeners.forEach((l) => l(true));
  setActivity("speech", true);
  await new Promise<void>((resolve) => {
    audio.onended = audio.onerror = audio.onpause = () => resolve();
    audio.play().catch(() => resolve());
  });
  URL.revokeObjectURL(url);
  if (current === audio) current = null;
  speakingListeners.forEach((l) => l(false));
  setActivity("speech", false);
}

// Dictée ou lecture en cours : le mot d'éveil n'est pas guetté pendant ce temps.
const activity = new Set<string>();
const activityListeners = new Set<(busy: boolean) => void>();

function setActivity(kind: string, on: boolean) {
  if (on) activity.add(kind);
  else activity.delete(kind);
  activityListeners.forEach((l) => l(activity.size > 0));
}

export function onVoiceActivity(l: (busy: boolean) => void): () => void {
  activityListeners.add(l);
  return () => activityListeners.delete(l);
}

/** Les réglages de la voix ont changé (l'écoute du mot d'éveil se met à jour). */
export const VOICE_SETTINGS_EVENT = "claudio:voice-settings";

export function stopSpeaking() {
  current?.pause();
  current = null;
}

const speakingListeners = new Set<(on: boolean) => void>();
export function onSpeaking(l: (on: boolean) => void): () => void {
  speakingListeners.add(l);
  return () => speakingListeners.delete(l);
}

/**
 * Demande vocale venue du mot d'éveil, en attente de la page Conversations :
 * `""` = le mot a été dit seul (il faut écouter la suite).
 */
let pendingCommand: string | null = null;
const commandListeners = new Set<() => void>();

export function pushVoiceCommand(text: string) {
  pendingCommand = text;
  commandListeners.forEach((l) => l());
}

export function takeVoiceCommand(): string | null {
  const c = pendingCommand;
  pendingCommand = null;
  return c;
}

export function onVoiceCommand(l: () => void): () => void {
  commandListeners.add(l);
  return () => commandListeners.delete(l);
}
