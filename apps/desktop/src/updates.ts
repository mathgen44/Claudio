// Mise à jour automatique : vérifie la dernière version publiée (signée), la
// télécharge, l'installe puis relance Claudio.

import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { inTauri } from "./api";

export type { Update };

/** Démarrage avec Windows (clé « Exécuter » de l'utilisateur). */
export async function autostartEnabled(): Promise<boolean> {
  if (!inTauri) return false;
  const { isEnabled } = await import("@tauri-apps/plugin-autostart");
  return isEnabled();
}

export async function setAutostart(on: boolean) {
  if (!inTauri) return;
  const { enable, disable } = await import("@tauri-apps/plugin-autostart");
  await (on ? enable() : disable());
}

/** Nouvelle version disponible, ou `null` (toujours `null` hors de l'application). */
export async function findUpdate(): Promise<Update | null> {
  if (!inTauri) return null;
  return check({ timeout: 20000 });
}

export async function installUpdate(update: Update, onProgress: (done: number, total: number | null) => void) {
  let done = 0;
  let total: number | null = null;
  await update.downloadAndInstall((e) => {
    if (e.event === "Started") total = e.data.contentLength ?? null;
    if (e.event === "Progress") done += e.data.chunkLength;
    onProgress(done, total);
  });
  await relaunch();
}
