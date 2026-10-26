// Cloud : fournisseurs distants, modèle de secours, routage, budgets et consommation.

import { KeyRound, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  api,
  formatUsd,
  routingLabels,
  type Config,
  type ProviderKind,
  type ProviderView,
  type RemoteModel,
  type RoutingMode,
  type UsageReport,
} from "../api";
import { Badge, Button, Card, ErrorNote, PageHeader, Spinner } from "../ui";

const KINDS: { kind: ProviderKind; label: string; hint: string }[] = [
  { kind: "anthropic", label: "Anthropic (Claude)", hint: "Clé sur console.anthropic.com" },
  { kind: "openai", label: "OpenAI", hint: "Clé sur platform.openai.com" },
  { kind: "mistral", label: "Mistral AI", hint: "Clé sur console.mistral.ai" },
  { kind: "gemini", label: "Google Gemini", hint: "Clé sur aistudio.google.com" },
  { kind: "openrouter", label: "OpenRouter", hint: "Accès à des centaines de modèles, prix affichés" },
  { kind: "custom", label: "Compatible OpenAI", hint: "LM Studio, vLLM, autre serveur (adresse requise)" },
];

const MODES: { mode: RoutingMode; text: string }[] = [
  { mode: "local", text: "Rien ne sort de la machine. Le cloud n'est jamais utilisé." },
  { mode: "auto", text: "Le modèle local répond ; le cloud prend le relais s'il échoue ou si la conversation est trop longue." },
  { mode: "cloud", text: "Le modèle distant répond ; le local sert de secours (budget atteint, panne)." },
];

const input = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";

export function Cloud() {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [usage, setUsage] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    try {
      const [c, p, u] = await Promise.all([api.config(), api.providers(), api.usageReport()]);
      setCfg(c);
      setProviders(p);
      setUsage(u);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const act = async (job: () => Promise<unknown>) => {
    setError(null);
    try {
      await job();
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  if (!cfg) return null;
  const cloud = cfg.routing.cloud;
  const cloudProvider = providers.find((p) => p.id === cloud?.provider_id);

  return (
    <div className="mx-auto max-w-4xl space-y-6 px-6 py-8">
      <PageHeader title="Cloud et budgets" subtitle="Modèles distants avec vos clés d'API, en complément des modèles locaux." />
      <ErrorNote message={error} />

      <Card className="space-y-3">
        <h2 className="font-medium">Routage</h2>
        <div className="grid gap-2 sm:grid-cols-3">
          {MODES.map(({ mode, text }) => (
            <button
              key={mode}
              onClick={() => act(() => api.setRoutingMode(mode))}
              className={`rounded-lg border p-3 text-left ${
                cfg.routing.mode === mode ? "border-[var(--accent)] bg-[var(--accent-soft)]/40" : "border-[var(--border)] hover:bg-[var(--panel-2)]"
              }`}
            >
              <div className="font-medium">{routingLabels[mode]}</div>
              <div className="mt-1 text-sm text-[var(--muted)]">{text}</div>
            </button>
          ))}
        </div>
        <p className="text-sm text-[var(--muted)]">
          Modèle distant :{" "}
          {cloud ? (
            <b className="text-[var(--text)]">
              {cloud.model} · {cloudProvider?.name ?? "fournisseur supprimé"}
            </b>
          ) : (
            "aucun — choisissez-en un dans un fournisseur ci-dessous"
          )}
          {cloud && (
            <button className="ml-2 text-[var(--accent)] underline" onClick={() => act(() => api.setCloudModel(null, null))}>
              retirer
            </button>
          )}
        </p>
        <p className="text-xs text-[var(--muted)]">Chaque conversation peut aussi choisir son propre mode depuis son en-tête.</p>
      </Card>

      <section className="space-y-3">
        <div className="flex items-center justify-between">
          <h2 className="font-medium">Fournisseurs</h2>
          {!adding && (
            <Button onClick={() => setAdding(true)}>
              <Plus size={16} /> Ajouter
            </Button>
          )}
        </div>
        {adding && <AddProvider onDone={() => (setAdding(false), load())} onError={setError} />}
        {providers.length === 0 && !adding && (
          <p className="text-[var(--muted)]">Aucun fournisseur. Claudio fonctionne alors uniquement en local.</p>
        )}
        {providers.map((p) => (
          <ProviderCard key={p.id} p={p} cfg={cfg} act={act} />
        ))}
      </section>

      {usage && <Usage report={usage} cfg={cfg} act={act} />}
    </div>
  );
}

function AddProvider({ onDone, onError }: { onDone: () => void; onError: (e: string) => void }) {
  const [kind, setKind] = useState<ProviderKind>("anthropic");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const info = KINDS.find((k) => k.kind === kind)!;

  const save = async () => {
    setBusy(true);
    try {
      await api.addProvider(kind, name, url.trim() || null, key.trim() || null);
      onDone();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card className="space-y-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="space-y-1">
          <span className="text-sm font-medium">Fournisseur</span>
          <select className={`${input} w-full`} value={kind} onChange={(e) => setKind(e.target.value as ProviderKind)}>
            {KINDS.map((k) => (
              <option key={k.kind} value={k.kind}>
                {k.label}
              </option>
            ))}
          </select>
          <span className="block text-xs text-[var(--muted)]">{info.hint}</span>
        </label>
        <label className="space-y-1">
          <span className="text-sm font-medium">Nom (facultatif)</span>
          <input className={`${input} w-full`} value={name} placeholder={info.label} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="space-y-1">
          <span className="text-sm font-medium">Clé d'API{kind === "custom" && " (facultative)"}</span>
          <input
            type="password"
            autoComplete="off"
            className={`${input} w-full`}
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
          <span className="block text-xs text-[var(--muted)]">Rangée chiffrée dans le Gestionnaire d'identifiants Windows.</span>
        </label>
        <label className="space-y-1">
          <span className="text-sm font-medium">Adresse de l'API{kind !== "custom" && " (facultative)"}</span>
          <input
            className={`${input} w-full`}
            value={url}
            placeholder={kind === "custom" ? "http://localhost:1234/v1" : "adresse par défaut"}
            onChange={(e) => setUrl(e.target.value)}
          />
        </label>
      </div>
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onDone}>
          Annuler
        </Button>
        <Button variant="primary" disabled={busy || (kind !== "custom" && !key.trim()) || (kind === "custom" && !url.trim())} onClick={save}>
          {busy ? <Spinner /> : "Ajouter"}
        </Button>
      </div>
    </Card>
  );
}

function ProviderCard({ p, cfg, act }: { p: ProviderView; cfg: Config; act: (job: () => Promise<unknown>) => Promise<void> }) {
  const [models, setModels] = useState<RemoteModel[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [newKey, setNewKey] = useState<string | null>(null);

  const loadModels = async () => {
    setLoading(true);
    setError(null);
    try {
      setModels(await api.providerModels(p.id));
    } catch (e) {
      setError(String(e));
      setModels(null);
    } finally {
      setLoading(false);
    }
  };

  const choose = (m: RemoteModel) =>
    act(() =>
      api.setCloudModel(
        { provider_id: p.id, model: m.id },
        m.input_per_m != null && m.output_per_m != null ? [m.input_per_m, m.output_per_m] : null,
      ),
    );

  const setPrice = (m: RemoteModel) => {
    const raw = window.prompt(`Prix de ${m.id} en $ par million de jetons : « entrée sortie » (ex. 2,5 10)`);
    const parts = raw?.replace(/,/g, ".").trim().split(/\s+/).map(Number);
    if (parts?.length === 2 && parts.every((n) => Number.isFinite(n) && n >= 0)) {
      act(() => api.setModelPrice(p.id, m.id, parts[0], parts[1])).then(loadModels);
    }
  };

  const shown = (models ?? []).filter((m) => m.id.toLowerCase().includes(filter.toLowerCase())).slice(0, 60);

  return (
    <Card className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{p.name}</span>
            <Badge>{p.kind_label}</Badge>
            {p.has_key ? <Badge tone="ok">clé enregistrée</Badge> : p.kind !== "custom" && <Badge>pas de clé</Badge>}
            {!p.enabled && <Badge>désactivé</Badge>}
          </div>
          <p className="truncate text-xs text-[var(--muted)]">{p.base_url ?? p.default_base_url}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <label className="mr-2 flex items-center gap-1.5 text-sm">
            <input
              type="checkbox"
              className="accent-[var(--accent)]"
              checked={p.enabled}
              onChange={(e) => act(() => api.updateProvider(p.id, { enabled: e.target.checked }))}
            />
            Activé
          </label>
          <Button variant="ghost" onClick={() => setNewKey("")} aria-label="Changer la clé">
            <KeyRound size={16} />
          </Button>
          <Button
            variant="danger"
            aria-label="Supprimer"
            onClick={() => window.confirm(`Supprimer ${p.name} et sa clé ?`) && act(() => api.removeProvider(p.id))}
          >
            <Trash2 size={16} />
          </Button>
        </div>
      </div>

      {newKey != null && (
        <div className="flex gap-2">
          <input
            type="password"
            autoComplete="off"
            className={`${input} flex-1`}
            placeholder="Nouvelle clé d'API"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
          />
          <Button disabled={!newKey.trim()} onClick={() => act(() => api.setProviderKey(p.id, newKey)).then(() => setNewKey(null))}>
            Enregistrer
          </Button>
          <Button variant="ghost" onClick={() => setNewKey(null)}>
            Annuler
          </Button>
        </div>
      )}

      <div className="flex items-center gap-2">
        <Button onClick={loadModels} disabled={loading}>
          {loading ? <Spinner /> : <RefreshCw size={16} />} {models ? "Actualiser" : "Tester et lister les modèles"}
        </Button>
        {models && (
          <input className={`${input} flex-1`} placeholder={`Filtrer ${models.length} modèles…`} value={filter} onChange={(e) => setFilter(e.target.value)} />
        )}
      </div>
      <ErrorNote message={error} />
      {models && (
        <div className="max-h-72 divide-y divide-[var(--border)] overflow-y-auto rounded-lg border border-[var(--border)]">
          {shown.map((m) => {
            const active = cfg.routing.cloud?.provider_id === p.id && cfg.routing.cloud.model === m.id;
            return (
              <div key={m.id} className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
                <div className="min-w-0">
                  <div className="truncate font-medium">{m.id}</div>
                  <button className="text-xs text-[var(--muted)] hover:text-[var(--accent)]" onClick={() => setPrice(m)}>
                    {m.input_per_m != null
                      ? `${formatUsd(m.input_per_m)} / ${formatUsd(m.output_per_m)} par million (entrée / sortie)`
                      : "prix inconnu — cliquer pour le saisir"}
                  </button>
                </div>
                {active ? <Badge tone="accent">Modèle distant</Badge> : <Button onClick={() => choose(m)}>Utiliser</Button>}
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}

function Usage({ report, cfg, act }: { report: UsageReport; cfg: Config; act: (job: () => Promise<unknown>) => Promise<void> }) {
  const [daily, setDaily] = useState(cfg.budget.daily_usd?.toString() ?? "");
  const [monthly, setMonthly] = useState(cfg.budget.monthly_usd?.toString() ?? "");
  const b = report.budget;
  const parse = (v: string) => (v.trim() === "" ? null : Number(v.replace(",", ".")));
  const valid = [daily, monthly].every((v) => v.trim() === "" || (Number.isFinite(parse(v)) && parse(v)! >= 0));
  const maxDay = Math.max(0.0001, ...report.daily.map((d) => d.cost_usd));

  return (
    <Card className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="font-medium">Budget et consommation</h2>
        {b.exceeded ? <Badge>Plafond atteint : cloud bloqué</Badge> : b.warning && <Badge>Plus de 80 % du plafond</Badge>}
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        <Meter label="Aujourd'hui" spent={b.today_usd} limit={b.daily_usd} />
        <Meter label="Ce mois-ci" spent={b.month_usd} limit={b.monthly_usd} />
      </div>
      <div className="flex flex-wrap items-end gap-3">
        <label className="space-y-1">
          <span className="text-sm font-medium">Plafond par jour ($)</span>
          <input className={`${input} w-32`} inputMode="decimal" placeholder="aucun" value={daily} onChange={(e) => setDaily(e.target.value)} />
        </label>
        <label className="space-y-1">
          <span className="text-sm font-medium">Plafond par mois ($)</span>
          <input className={`${input} w-32`} inputMode="decimal" placeholder="aucun" value={monthly} onChange={(e) => setMonthly(e.target.value)} />
        </label>
        <Button disabled={!valid} onClick={() => act(() => api.setBudget({ daily_usd: parse(daily), monthly_usd: parse(monthly) }))}>
          Enregistrer
        </Button>
      </div>

      {report.daily.length > 0 && (
        <div>
          <div className="mb-1 text-sm text-[var(--muted)]">Dépense cloud des 30 derniers jours</div>
          <div className="flex h-20 items-end gap-1" role="img" aria-label="Dépense par jour">
            {report.daily.map((d) => (
              <div
                key={d.day}
                title={`${d.day} : ${formatUsd(d.cost_usd)} · ${d.cloud_requests} demandes cloud, ${d.local_requests} locales`}
                className="min-w-1 flex-1 rounded-t bg-[var(--accent)]"
                style={{ height: `${Math.max(2, (d.cost_usd / maxDay) * 100)}%`, opacity: d.cost_usd > 0 ? 1 : 0.25 }}
              />
            ))}
          </div>
        </div>
      )}

      <table className="w-full text-sm">
        <thead className="text-left text-[var(--muted)]">
          <tr>
            <th className="py-1 font-normal">Modèle (ce mois)</th>
            <th className="py-1 text-right font-normal">Demandes</th>
            <th className="py-1 text-right font-normal">Jetons entrée / sortie</th>
            <th className="py-1 text-right font-normal">Coût</th>
          </tr>
        </thead>
        <tbody>
          {report.month.length === 0 && (
            <tr>
              <td colSpan={4} className="py-2 text-[var(--muted)]">
                Aucune demande ce mois-ci.
              </td>
            </tr>
          )}
          {report.month.map((r) => (
            <tr key={`${r.provider}/${r.model}`} className="border-t border-[var(--border)]">
              <td className="py-1.5">
                {r.model} <span className="text-[var(--muted)]">· {r.local ? "local" : r.provider}</span>
              </td>
              <td className="py-1.5 text-right tabular-nums">{r.requests}</td>
              <td className="py-1.5 text-right tabular-nums">
                {r.input_tokens.toLocaleString("fr-FR")} / {r.output_tokens.toLocaleString("fr-FR")}
              </td>
              <td className="py-1.5 text-right tabular-nums">
                {r.local ? "gratuit" : formatUsd(r.cost_usd)}
                {r.unpriced > 0 && <span title="Prix inconnu pour certaines demandes"> *</span>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {report.month.some((r) => r.unpriced > 0) && (
        <p className="text-xs text-[var(--muted)]">* prix du modèle inconnu : coût sous-estimé. Saisissez-le dans la liste des modèles du fournisseur.</p>
      )}
    </Card>
  );
}

function Meter({ label, spent, limit }: { label: string; spent: number; limit: number | null }) {
  const pct = limit ? Math.min(100, (spent / Math.max(limit, 1e-9)) * 100) : 0;
  return (
    <div className="space-y-1.5">
      <div className="flex items-baseline justify-between text-sm">
        <span className="text-[var(--muted)]">{label}</span>
        <span className="tabular-nums">
          <b>{formatUsd(spent)}</b>
          {limit != null && <span className="text-[var(--muted)]"> / {formatUsd(limit)}</span>}
        </span>
      </div>
      {limit != null && (
        <div className="h-2 overflow-hidden rounded-full bg-[var(--panel-2)]">
          <div
            className={`h-full rounded-full ${pct >= 100 ? "bg-[var(--danger)]" : pct >= 80 ? "bg-amber-500" : "bg-[var(--accent)]"}`}
            style={{ width: `${pct}%` }}
          />
        </div>
      )}
    </div>
  );
}
