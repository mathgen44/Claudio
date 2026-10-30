// Outils et accès : dossiers autorisés, règles des outils, serveurs MCP.

import { FolderOpen, FolderPlus, KeyRound, Plug, Plus, ShieldCheck, Trash2, X } from "lucide-react";
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

/** Serveur MCP connu, proposé prérempli ; les valeurs en MAJUSCULES sont à remplacer. */
type McpPreset = { name: string; description: string; command: string; args: string[]; keys: string[]; help: string };

const MCP_PRESETS: McpPreset[] = [
  {
    name: "SSH",
    description: "Commandes sur un serveur distant",
    command: "npx",
    args: ["-y", "ssh-mcp", "--host=ADRESSE", "--user=UTILISATEUR", "--key=C:\\Users\\VOUS\\.ssh\\id_ed25519"],
    keys: [],
    help: "Remplacez ADRESSE, UTILISATEUR et le chemin de la clé privée. Sans clé, retirez --key et ajoutez la clé SSH_MCP_PASSWORD (mot de passe). La première connexion mémorise l'empreinte du serveur.",
  },
  {
    name: "GitHub",
    description: "Dépôts, tickets, demandes de fusion",
    command: "docker",
    args: ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"],
    keys: ["GITHUB_PERSONAL_ACCESS_TOKEN"],
    help: "Serveur officiel de GitHub, lancé avec Docker. Créez un jeton dans GitHub › Settings › Developer settings › Personal access tokens, avec les seuls droits utiles.",
  },
  {
    name: "Git",
    description: "Historique et différences d'un dépôt local",
    command: "uvx",
    args: ["mcp-server-git", "--repository", "C:\\CHEMIN\\DU\\DEPOT"],
    keys: [],
    help: "Remplacez le chemin par celui de votre dépôt. Nécessite uv (installé sur cette machine).",
  },
  {
    name: "Navigateur",
    description: "Pilote un navigateur (Playwright)",
    command: "npx",
    args: ["-y", "@playwright/mcp"],
    keys: [],
    help: "Serveur de Microsoft : l'assistant ouvre des pages, clique et remplit des formulaires. Les navigateurs sont téléchargés au premier lancement.",
  },
  {
    name: "Notion",
    description: "Pages et bases Notion",
    command: "npx",
    args: ["-y", "@notionhq/notion-mcp-server"],
    keys: ["NOTION_TOKEN"],
    help: "Créez une intégration interne sur notion.so/profile/integrations, copiez son jeton (ntn_…) et partagez avec elle les pages à rendre accessibles.",
  },
  {
    name: "Brave Search",
    description: "Recherche web par l'API Brave",
    command: "npx",
    args: ["-y", "@brave/brave-search-mcp-server", "--transport", "stdio"],
    keys: ["BRAVE_API_KEY"],
    help: "Clé gratuite (quota mensuel) sur brave.com/search/api.",
  },
  {
    name: "Heure",
    description: "Heure et conversions de fuseaux",
    command: "uvx",
    args: ["mcp-server-time", "--local-timezone=Europe/Paris"],
    keys: [],
    help: "Aucun réglage nécessaire.",
  },
];

/** Clé d'un serveur MCP en cours de saisie ; `saved` : une valeur est déjà enregistrée. */
type KeyRow = { key: string; value: string; saved: boolean };

const KEY_NAME = /^[A-Za-z0-9_-]+$/;
const keysValid = (rows: KeyRow[]) =>
  rows.every((r) => KEY_NAME.test(r.key) && (r.saved || r.value !== "")) &&
  new Set(rows.map((r) => r.key)).size === rows.length;
const keyPairs = (rows: KeyRow[]): [string, string][] => rows.map((r) => [r.key, r.value]);

/** Clés transmises au serveur : variables d'environnement (programme local) ou
 * en-têtes (serveur distant). Les valeurs vont au Gestionnaire d'identifiants. */
function KeyEditor({ kind, rows, onChange }: { kind: "stdio" | "http"; rows: KeyRow[]; onChange: (rows: KeyRow[]) => void }) {
  const set = (i: number, patch: Partial<KeyRow>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm">
          {kind === "stdio" ? "Clés (variables d'environnement)" : "Clés (en-têtes HTTP)"}
        </span>
        <Button variant="ghost" className="px-2 py-1" onClick={() => onChange([...rows, { key: "", value: "", saved: false }])}>
          <Plus size={14} /> Ajouter une clé
        </Button>
      </div>
      {rows.map((r, i) => (
        <div key={i} className="flex gap-2">
          <input
            className={`${input} w-2/5 font-mono text-sm`}
            placeholder={kind === "stdio" ? "GITHUB_PERSONAL_ACCESS_TOKEN" : "Authorization"}
            value={r.key}
            disabled={r.saved}
            onChange={(e) => set(i, { key: e.target.value.trim() })}
          />
          <input
            type="password"
            autoComplete="off"
            className={`${input} min-w-0 flex-1 font-mono text-sm`}
            placeholder={r.saved ? "enregistrée — laisser vide pour la garder" : kind === "stdio" ? "valeur" : "Bearer …"}
            value={r.value}
            onChange={(e) => set(i, { value: e.target.value })}
          />
          <Button variant="danger" aria-label="Retirer la clé" onClick={() => onChange(rows.filter((_, j) => j !== i))}>
            <X size={16} />
          </Button>
        </div>
      ))}
      {rows.some((r) => r.key && !KEY_NAME.test(r.key)) && (
        <p className="text-xs text-[var(--danger)]">Nom de clé : lettres sans accents, chiffres, _ et - uniquement.</p>
      )}
      <p className="text-xs text-[var(--muted)]">
        Les valeurs sont rangées dans le Gestionnaire d'identifiants de Windows, jamais dans les fichiers de Claudio ni dans
        les journaux.
      </p>
    </div>
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
  const [keys, setKeys] = useState<KeyRow[]>([]);
  const [preset, setPreset] = useState<McpPreset | null>(null);
  const [editingKeys, setEditingKeys] = useState<{ id: string; rows: KeyRow[] } | null>(null);
  const [tools, setTools] = useState<Record<string, McpTool[] | "loading" | string>>({});

  const choose = (p: McpPreset) => {
    setPreset(p);
    setKind("stdio");
    setName(p.name);
    setCommand(p.command);
    setArgs(p.args.join("\n"));
    setKeys(p.keys.map((key) => ({ key, value: "", saved: false })));
    setAdding(true);
  };

  const add = async () => {
    await act(() =>
      api.addMcpServer(
        name,
        kind === "stdio"
          ? { type: "stdio", command: command.trim(), args: args.split("\n").map((a) => a.trim()).filter(Boolean), env: [] }
          : { type: "http", url: url.trim() },
        keyPairs(keys),
      ),
    );
    setAdding(false);
    setName("");
    setCommand("");
    setArgs("");
    setUrl("");
    setKeys([]);
    setPreset(null);
  };

  const saveKeys = async () => {
    if (!editingKeys) return;
    await act(() => api.setMcpSecrets(editingKeys.id, keyPairs(editingKeys.rows)));
    setEditingKeys(null);
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
      {!adding && (
        <div className="space-y-2">
          <div className="text-sm">Connecteurs usuels</div>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {MCP_PRESETS.map((p) => (
              <button
                key={p.name}
                className="rounded-lg border border-[var(--border)] p-2.5 text-left hover:bg-[var(--panel-2)]"
                onClick={() => choose(p)}
              >
                <div className="text-sm font-medium">{p.name}</div>
                <div className="text-xs text-[var(--muted)]">{p.description}</div>
              </button>
            ))}
          </div>
        </div>
      )}
      {adding && (
        <div className="space-y-3 rounded-lg border border-[var(--border)] p-3">
          {preset && <p className="rounded-lg bg-[var(--panel-2)] p-2.5 text-sm">{preset.help}</p>}
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
          <KeyEditor kind={kind} rows={keys} onChange={setKeys} />
          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              onClick={() => {
                setAdding(false);
                setPreset(null);
              }}
            >
              Annuler
            </Button>
            <Button
              variant="primary"
              disabled={(kind === "stdio" ? !command.trim() : !url.trim()) || !keysValid(keys)}
              onClick={add}
            >
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
                <Button
                  onClick={() =>
                    setEditingKeys(
                      editingKeys?.id === s.id ? null : { id: s.id, rows: s.secrets.map((k) => ({ key: k, value: "", saved: true })) },
                    )
                  }
                >
                  <KeyRound size={16} /> Clés{s.secrets.length > 0 && ` (${s.secrets.length})`}
                </Button>
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
            {editingKeys?.id === s.id && (
              <div className="space-y-2 rounded-lg bg-[var(--panel-2)] p-3">
                <KeyEditor
                  kind={s.transport.type}
                  rows={editingKeys.rows}
                  onChange={(rows) => setEditingKeys({ id: s.id, rows })}
                />
                <div className="flex justify-end gap-2">
                  <Button variant="ghost" onClick={() => setEditingKeys(null)}>
                    Annuler
                  </Button>
                  <Button variant="primary" disabled={!keysValid(editingKeys.rows)} onClick={saveKeys}>
                    Enregistrer les clés
                  </Button>
                </div>
              </div>
            )}
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
