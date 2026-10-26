import { Cpu, Gpu, MemoryStick } from "lucide-react";
import type { Status } from "../api";
import { Badge, Card, formatBytes } from "../ui";

const tierText: Record<string, string> = {
  S: "Machine légère : petits modèles rapides, un agent à la fois.",
  M: "Machine intermédiaire : modèles jusqu'à ~9 milliards de paramètres.",
  L: "Machine puissante : modèles de 12 à 24 milliards de paramètres.",
  XL: "Machine très puissante : les plus gros modèles locaux.",
};

/** Résumé du matériel détecté et du profil retenu. */
export function MachineCard({ status }: { status: Status }) {
  const { hardware: hw, profile } = status;
  const gpu = profile.gpu_index != null ? hw.gpus[profile.gpu_index] : null;
  return (
    <Card>
      <div className="mb-4 flex items-center gap-3">
        <Badge tone="accent">Profil {profile.tier}</Badge>
        <span className="text-[var(--muted)]">{tierText[profile.tier]}</span>
      </div>
      <dl className="grid gap-4 sm:grid-cols-3">
        <Item icon={<Gpu size={18} />} label="Carte graphique">
          {gpu ? (
            <>
              {gpu.name}
              <div className="text-[var(--muted)]">
                {formatBytes(gpu.vram_total)} · {formatBytes(profile.vram_budget)} disponibles pour un modèle
              </div>
            </>
          ) : (
            <span className="text-[var(--muted)]">Aucune carte dédiée : le processeur fera le travail</span>
          )}
        </Item>
        <Item icon={<Cpu size={18} />} label="Processeur">
          {hw.cpu.name}
          <div className="text-[var(--muted)]">
            {hw.cpu.physical_cores} cœurs · {hw.cpu.logical_cores} threads
          </div>
        </Item>
        <Item icon={<MemoryStick size={18} />} label="Mémoire vive">
          {formatBytes(hw.ram_total)}
          <div className="text-[var(--muted)]">{formatBytes(hw.ram_available)} libres</div>
        </Item>
      </dl>
    </Card>
  );
}

function Item({ icon, label, children }: { icon: React.ReactNode; label: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-3">
      <div className="mt-0.5 text-[var(--accent)]">{icon}</div>
      <div>
        <dt className="text-xs tracking-wide text-[var(--muted)] uppercase">{label}</dt>
        <dd className="mt-0.5">{children}</dd>
      </div>
    </div>
  );
}
