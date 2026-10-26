// Journal d'audit : chaque action des outils, avec la décision et le résultat.

import { ShieldAlert, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, toolLabel, type AuditEntry } from "../api";
import { Badge, Button, Card, ErrorNote, PageHeader } from "../ui";

const PAGE = 100;

export function Journal() {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [more, setMore] = useState(false);
  const [broken, setBroken] = useState<number | null | undefined>(undefined);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (before: number | null) => {
    try {
      const page = await api.auditLog(PAGE, before);
      setEntries((e) => (before == null ? page : [...e, ...page]));
      setMore(page.length === PAGE);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    load(null);
    api.verifyAudit().then(setBroken, (e) => setError(String(e)));
  }, [load]);

  const f = filter.toLowerCase();
  const shown = entries.filter(
    (e) => !f || [e.tool, e.detail, e.decision, e.outcome].some((x) => x.toLowerCase().includes(f)),
  );

  return (
    <div className="mx-auto max-w-5xl space-y-5 px-6 py-8">
      <PageHeader
        title="Journal d'activité"
        subtitle="Toutes les actions des outils, en ajout seul. Chaque entrée est chaînée à la précédente."
        actions={
          broken === undefined ? null : broken == null ? (
            <Badge tone="ok">
              <ShieldCheck size={13} className="mr-1" /> Journal intact
            </Badge>
          ) : (
            <span className="flex items-center gap-1 text-sm text-[var(--danger)]">
              <ShieldAlert size={16} /> Journal altéré à partir de l'entrée {broken}
            </span>
          )
        }
      />
      <ErrorNote message={error} />
      <input
        className="w-full rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2"
        placeholder="Filtrer (outil, chemin, décision…)"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />
      <Card className="overflow-x-auto p-0">
        <table className="w-full text-sm">
          <thead className="text-left text-[var(--muted)]">
            <tr className="border-b border-[var(--border)]">
              <th className="px-3 py-2 font-normal">Date</th>
              <th className="px-3 py-2 font-normal">Outil</th>
              <th className="px-3 py-2 font-normal">Détail</th>
              <th className="px-3 py-2 font-normal">Décision</th>
              <th className="px-3 py-2 font-normal">Résultat</th>
            </tr>
          </thead>
          <tbody>
            {shown.length === 0 && (
              <tr>
                <td colSpan={5} className="px-3 py-6 text-center text-[var(--muted)]">
                  Aucune action pour l'instant.
                </td>
              </tr>
            )}
            {shown.map((e) => (
              <tr key={e.id} className="border-b border-[var(--border)] align-top last:border-0">
                <td className="px-3 py-2 whitespace-nowrap text-[var(--muted)] tabular-nums">
                  {new Date(e.at).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "medium" })}
                </td>
                <td className="px-3 py-2 whitespace-nowrap">{toolLabel(e.tool)}</td>
                <td className="max-w-md px-3 py-2 font-mono text-xs break-all">{e.detail}</td>
                <td className="px-3 py-2 text-xs">
                  <span className={e.decision.includes("refus") ? "text-[var(--danger)]" : ""}>{e.decision}</span>
                </td>
                <td className="px-3 py-2 text-xs">
                  {e.outcome === "ok" ? <span className="text-[var(--ok)]">ok</span> : <span className="text-[var(--danger)]">{e.outcome}</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      {more && (
        <div className="flex justify-center">
          <Button onClick={() => load(entries[entries.length - 1].id)}>Plus ancien</Button>
        </div>
      )}
    </div>
  );
}
