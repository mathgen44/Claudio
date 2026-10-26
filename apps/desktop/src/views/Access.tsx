// Outils et accès : dossiers autorisés, règles des outils, serveurs MCP.

import { FolderOpen, FolderPlus, Plug, Plus, ShieldCheck, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, type AccessOverview, type McpServer, type McpTool, type Permissions, type Policy } from "../api";
import { Badge, Button, Card, ErrorNote, PageHeader, Spinner } from "../ui";

const input = "rounded-lg border border-[var(--border)] bg-[var(--panel)] px-3 py-2";

const POLICY: Record<Policy, string> = { ask: "Demander", allow: "Autoriser", deny: "Refuser" };

export function Access() {
  const [data, setData] = useState<AccessOverview | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api.accessOverview());
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

  if (!data) return null;
  const p = data.permissions;
  const setP = (patch: Partial<Permissions>) => act(() => api.setPermissions({ ...p, ...patch }));
  const addFolder = async (write: boolean) => {
    const path = await api.pickFolder();
    if (path) act(() => api.grantFolder(path, write));
  };
  const folders = data.grants.filter((g) => g.kind === "folder");
  const others = data.grants.filter((g) => g.kind !== "folder");

  return (
    <div className="mx-auto max-w-4xl space-y-6 px-6 py-8">
      <PageHeader
        title="Outils et accès"
        subtitle="Ce que l'assistant peut faire sur cet ordinateur. Rien n'est accessible sans votre accord."
      />
      <ErrorNote message={error} />

      <Card className="flex items-center justify-between gap-4">
        <div>
          <div className="font-medium">Outils de l'assistant</div>
          <div className="text-sm text-[var(--muted)]">
            Fichiers, web, PowerShell et serveurs MCP. Désactivés, l'assistant ne fait que discuter.
          </div>
        </div>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            className="size-4 accent-[var(--accent)]"
            checked={data.tools_enabled}
            onChange={(e) => act(() => api.setToolsEnabled(e.target.checked))}
          />
          Activés
        </label>
      </Card>

      <Card className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <h2 className="font-medium">Dossiers autorisés</h2>
          <div className="flex gap-2">
            <Button onClick={() => addFolder(false)}>
              <FolderPlus size={16} /> Lecture seule
            </Button>
            <Button onClick={() => addFolder(true)}>
              <FolderPlus size={16} /> Lecture et écriture
            </Button>
          </div>
        </div>
        <div className="divide-y divide-[var(--border)] rounded-lg border border-[var(--border)]">
          <div className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
            <div className="min-w-0">
              <div className="truncate font-medium">{data.workspace}</div>
              <div className="text-xs text-[var(--muted)]">Espace de travail de Claudio : toujours accessible</div>
            </div>
            <div className="flex items-center gap-2">
              <Badge tone="ok">lecture et écriture</Badge>
              <Button variant="ghost" aria-label="Ouvrir l'espace de travail" onClick={() => api.openFolder("workspace")}>
                <FolderOpen size={16} />
              </Button>
            </div>
          </div>
          {folders.map((g) => (
            <div key={g.id} className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
              <span className="min-w-0 truncate">{g.target}</span>
              <div className="flex items-center gap-2">
                <Badge tone={g.write ? "accent" : "neutral"}>{g.write ? "lecture et écriture" : "lecture"}</Badge>
                <Button variant="danger" aria-label="Retirer" onClick={() => act(() => api.revokeGrant(g.id))}>
                  <Trash2 size={16} />
                </Button>
              </div>
            </div>
          ))}
        </div>
        <p className="text-xs text-[var(--muted)]">
          Les autorisations « pour cette conversation » ne sont pas listées : elles disparaissent à la fermeture. Les dossiers
          sensibles (clés SSH, profils de navigateurs, données de Claudio) restent toujours inaccessibles. Supprimer un
          fichier, remplacer un fichier existant et exécuter une commande sont toujours confirmés.
        </p>
        {others.length > 0 && (
          <div className="space-y-1">
            <div className="text-sm font-medium">Sites et outils MCP toujours autorisés</div>
            {others.map((g) => (
              <div key={g.id} className="flex items-center justify-between gap-3 text-sm">
                <span className="truncate">
                  <Badge>{g.kind === "host" ? "site" : "MCP"}</Badge> {g.target}
                </span>
                <Button variant="danger" aria-label="Retirer" onClick={() => act(() => api.revokeGrant(g.id))}>
                  <Trash2 size={16} />
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card className="space-y-4">
        <h2 className="flex items-center gap-2 font-medium">
          <ShieldCheck size={18} /> Règles
        </h2>
        <Rule label="Sites web" hint="Quand l'assistant veut lire une page ou faire une recherche.">
          <PolicySelect value={p.network} onChange={(network) => setP({ network })} />
        </Rule>
        <Rule label="Commandes PowerShell" hint="Chaque commande vous est toujours montrée avant exécution.">
          <PolicySelect value={p.shell} options={["ask", "deny"]} onChange={(shell) => setP({ shell })} />
        </Rule>
        <Rule
          label="Bac à sable pour les commandes"
          hint="Les commandes ne peuvent écrire que dans l'espace de travail. Les désactiver leur donne tous vos droits."
        >
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="accent-[var(--accent)]"
              checked={p.shell_sandbox}
              onChange={(e) =>
                (e.target.checked ||
                  window.confirm("Sans bac à sable, une commande peut modifier n'importe lequel de vos fichiers. Continuer ?")) &&
                setP({ shell_sandbox: e.target.checked })
              }
            />
            {p.shell_sandbox ? "Activé" : <span className="text-[var(--danger)]">Désactivé</span>}
          </label>
        </Rule>
        <Rule label="Outils MCP" hint="Appels aux outils des serveurs MCP configurés ci-dessous.">
          <PolicySelect value={p.mcp} onChange={(mcp) => setP({ mcp })} />
        </Rule>
        <Rule label="Réseau local" hint="Laisser l'outil web joindre les adresses locales (box, NAS, serveurs maison).">
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="accent-[var(--accent)]"
              checked={p.allow_private_network}
              onChange={(e) => setP({ allow_private_network: e.target.checked })}
            />
            Autorisé
          </label>
        </Rule>
      </Card>

      <McpServers servers={data.mcp_servers} act={act} onError={setError} />
    </div>
  );
}

function Rule({ label, hint, children }: { label: string; hint: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="font-medium">{label}</div>
        <div className="text-sm text-[var(--muted)]">{hint}</div>
      </div>
      {children}
    </div>
  );
}

function PolicySelect({
  value,
  onChange,
  options = ["ask", "allow", "deny"],
}: {
  value: Policy;
  onChange: (p: Policy) => void;
  options?: Policy[];
}) {
  return (
    <select className={input} value={value} onChange={(e) => onChange(e.target.value as Policy)}>
      {options.map((o) => (
        <option key={o} value={o}>
          {POLICY[o]}
        </option>
      ))}
    </select>
  );
}

function McpServers({
  servers,
  act,
  onError,
}: {
  servers: McpServer[];
  act: (job: () => Promise<unknown>) => Promise<void>;
  onError: (e: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [kind, setKind] = useState<"stdio" | "http">("stdio");
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");
  const [tools, setTools] = useState<Record<string, McpTool[] | "loading" | string>>({});

  const add = async () => {
    await act(() =>
      api.addMcpServer(
        name,
        kind === "stdio"
          ? { type: "stdio", command: command.trim(), args: args.split("\n").map((a) => a.trim()).filter(Boolean), env: [] }
          : { type: "http", url: url.trim() },
      ),
    );
    setAdding(false);
    setName("");
    setCommand("");
    setArgs("");
    setUrl("");
  };

  const test = async (id: string) => {
    setTools((t) => ({ ...t, [id]: "loading" }));
    try {
      const list = await api.testMcpServer(id);
      setTools((t) => ({ ...t, [id]: list }));
    } catch (e) {
      setTools((t) => ({ ...t, [id]: String(e) }));
      onError(String(e));
    }
  };

  return (
    <Card className="space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="flex items-center gap-2 font-medium">
          <Plug size={18} /> Serveurs MCP
        </h2>
        {!adding && (
          <Button onClick={() => setAdding(true)}>
            <Plus size={16} /> Ajouter
          </Button>
        )}
      </div>
      <p className="text-sm text-[var(--muted)]">
        Outils externes au format Model Context Protocol (GitHub, bases de données, domotique…). Un serveur local est un
        programme lancé par Claudio : n'ajoutez que des serveurs de confiance.
      </p>
      {adding && (
        <div className="space-y-3 rounded-lg border border-[var(--border)] p-3">
          <div className="flex gap-2">
            <select className={input} value={kind} onChange={(e) => setKind(e.target.value as "stdio" | "http")}>
              <option value="stdio">Programme local</option>
              <option value="http">Serveur distant (HTTP)</option>
            </select>
            <input className={`${input} flex-1`} placeholder="Nom" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          {kind === "stdio" ? (
            <>
              <input
                className={`${input} w-full font-mono text-sm`}
                placeholder="Commande, ex. npx ou C:\\outils\\serveur.exe"
                value={command}
                onChange={(e) => setCommand(e.target.value)}
              />
              <textarea
                className={`${input} min-h-20 w-full font-mono text-sm`}
                placeholder={"Arguments, un par ligne, ex.\n-y\n@modelcontextprotocol/server-github"}
                value={args}
                onChange={(e) => setArgs(e.target.value)}
              />
            </>
          ) : (
            <input
              className={`${input} w-full font-mono text-sm`}
              placeholder="https://exemple.com/mcp"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
            />
          )}
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setAdding(false)}>
              Annuler
            </Button>
            <Button variant="primary" disabled={kind === "stdio" ? !command.trim() : !url.trim()} onClick={add}>
              Ajouter
            </Button>
          </div>
        </div>
      )}
      {servers.length === 0 && !adding && <p className="text-sm text-[var(--muted)]">Aucun serveur configuré.</p>}
      {servers.map((s) => {
        const t = tools[s.id];
        return (
          <div key={s.id} className="space-y-2 rounded-lg border border-[var(--border)] p-3">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <div className="font-medium">{s.name}</div>
                <div className="truncate font-mono text-xs text-[var(--muted)]">
                  {s.transport.type === "stdio" ? [s.transport.command, ...s.transport.args].join(" ") : s.transport.url}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <label className="flex items-center gap-1.5 text-sm">
                  <input
                    type="checkbox"
                    className="accent-[var(--accent)]"
                    checked={s.enabled}
                    onChange={(e) => act(() => api.setMcpEnabled(s.id, e.target.checked))}
                  />
                  Activé
                </label>
                <Button onClick={() => test(s.id)} disabled={t === "loading"}>
                  {t === "loading" ? <Spinner /> : "Tester"}
                </Button>
                <Button
                  variant="danger"
                  aria-label="Supprimer"
                  onClick={() => window.confirm(`Supprimer le serveur ${s.name} ?`) && act(() => api.removeMcpServer(s.id))}
                >
                  <Trash2 size={16} />
                </Button>
              </div>
            </div>
            {Array.isArray(t) && (
              <div className="flex flex-wrap gap-1.5">
                {t.length === 0 && <span className="text-sm text-[var(--muted)]">Aucun outil proposé.</span>}
                {t.map((tool) => (
                  <span key={tool.name} title={tool.description}>
                    <Badge tone="accent">{tool.name}</Badge>
                  </span>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </Card>
  );
}
