# Claudio

Assistant personnel pour Windows, **local d'abord** : il choisit, télécharge et fait
tourner des modèles de langage sur ta machine, en s'adaptant à sa puissance.

Voir [PLAN.md](PLAN.md) pour la vision complète et la feuille de route.

## État actuel (phases 0 à 5)

- Détection du matériel (processeur, mémoire, cartes graphiques via DXGI et NVML) et profil de machine S / M / L / XL.
- Catalogue de modèles GGUF (Qwen 3.5, Gemma 4, Qwen 3.6/3.8…) classés selon ce qui tient en mémoire.
- Téléchargement depuis Hugging Face avec reprise après coupure et vérification SHA-256.
- Installation automatique de llama.cpp (CUDA, Vulkan ou processeur selon la carte), chargement auto-ajusté à la mémoire libre.
- Réutilisation d'Ollama s'il est installé.
- Conversations en continu, historique SQLite, réflexion (« thinking ») affichable.
- Écrans : Mise en route, Conversations, Modèles, Cloud et budgets, Paramètres. Jetons rangés dans le Gestionnaire d'identifiants Windows.
- Recherche sur Hugging Face : tout dépôt GGUF, variantes découpées, estimation GPU / hybride / processeur (métadonnées GGUF, détection des mélanges d'experts), licence à accepter si elle n'est pas libre.
- Fournisseurs distants : Anthropic (API Messages native), OpenAI, Mistral, Gemini, OpenRouter et toute API compatible OpenAI.
- Routeur local / automatique / cloud, réglable par conversation, avec repli quand une cible échoue avant de répondre.
- Coûts par message, consommation par modèle, plafonds journalier et mensuel qui bloquent le cloud.
- Outils de l'assistant : fichiers (lister, lire, chercher, écrire, déplacer, corbeille), web (page, recherche), PowerShell, serveurs MCP (stdio et HTTP).
- Permissions : rien n'est accessible par défaut ; dossiers en lecture ou lecture/écriture, sites, outils MCP, accordés une fois, pour la conversation ou toujours. Suppression, remplacement de fichier (avec sauvegarde) et commandes toujours confirmés ; emplacements sensibles inaccessibles ; réseau local refusé par défaut.
- Bac à sable des commandes : PowerShell en intégrité basse (écriture limitée à l'espace de travail), job Windows (délai, mémoire, processus), variables secrètes retirées.
- Journal d'activité en ajout seul, chaîné par empreintes SHA-256, consultable dans l'application.
- Agents décrits en YAML (7 fournis, personnalisables dans `%APPDATA%\Claudio\agents`) : consignes, outils autorisés, modèle préféré.
- Tâches de fond : le planificateur découpe l'objectif en graphe d'étapes, exécutées en parallèle selon la machine, puis vérifiées et synthétisées ; reprise après interruption.
- Planifications cron, notifications Windows, icône dans la zone de notification (fermer la fenêtre ne coupe pas les tâches).
- Mémoire à long terme : souvenirs rappelés dans les réponses, retenus automatiquement (modèle local) ou à la demande, consultables, modifiables, épinglables et supprimables.
- Index des dossiers autorisés (texte, PDF, Word, Excel, PowerPoint, OpenDocument) : recherche hybride sens + mots (Qwen3-Embedding local, sqlite-vec, FTS5), mise à jour automatique à chaque modification.
- Compétences apprises : après une tâche réussie, une procédure réutilisable est proposée ; validée, l'assistant s'en sert.

## Développement

Prérequis : Rust (stable, MSVC), Node 20+, Visual Studio Build Tools, WebView2 (inclus dans Windows 11).

```bash
npm --prefix apps/desktop install
```

Lancer l'application :

```bash
npm --prefix apps/desktop run tauri dev
```

Interface seule dans un navigateur, avec des données simulées (`?ready` pour sauter la mise en route) :

```bash
npm --prefix apps/desktop run dev
```

Piloter le cœur sans interface :

```bash
cargo run -p claudio-app --example cli -- recommend
```

Autres commandes de l'outil : `status`, `runtime`, `install <id> <quant>`, `use <modèle>`, `search <texte>`, `repo <auteur/dépôt>`, `usage`, `chat "<texte>"`, `task "<objectif>"`, `index <dossier>`, `find "<question>"`, `memories`, `forget all`, `unindex <id>`.

Tests et vérifications (identiques à la CI) :

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Produire l'installateur Windows (MSI et NSIS dans `target/release/bundle`) :

```bash
npm --prefix apps/desktop run tauri build
```

Mettre à jour le catalogue de modèles après avoir modifié `catalog/sources.json` :

```bash
python -I scripts/update_catalog.py
```

## Organisation

| Dossier | Rôle |
|---|---|
| `crates/claudio-core` | Chemins, configuration TOML, journaux, secrets Windows, base SQLite |
| `crates/claudio-hw` | Profileur matériel |
| `crates/claudio-models` | Catalogue, choix du modèle, Hugging Face, téléchargements, registre |
| `crates/claudio-llm` | llama.cpp (installation, processus), Ollama, flux OpenAI et Anthropic, fournisseurs distants |
| `crates/claudio-perms` | Moteur de permissions, normalisation des chemins, bac à sable PowerShell |
| `crates/claudio-tools` | Outils (fichiers, web, PowerShell) et client MCP |
| `crates/claudio-agents` | Définitions d'agents (YAML) et plans en graphe |
| `crates/claudio-docs` | Extraction de texte (PDF, Office, OpenDocument, HTML) et découpage en passages |
| `crates/claudio-memory` | Index hybride, souvenirs, compétences |
| `crates/claudio-app` | Services partagés par l'application et l'outil : routeur, boucle d'agent, tâches, planifications, hub, cloud, accès |
| `agents/` | Agents fournis |
| `apps/desktop` | Application Tauri 2 (React + TypeScript + Tailwind) |
| `catalog/` | Catalogue des modèles recommandés |

## Données sur la machine

| Emplacement | Contenu |
|---|---|
| `%APPDATA%\Claudio\config.toml` | Configuration |
| `%LOCALAPPDATA%\Claudio\claudio.db` | Conversations |
| `%LOCALAPPDATA%\Claudio\models` | Modèles téléchargés |
| `%LOCALAPPDATA%\Claudio\runtimes` | Moteurs llama.cpp |
| `%LOCALAPPDATA%\Claudio\logs` | Journaux (niveau réglable avec `CLAUDIO_LOG`) |

La variable `CLAUDIO_HOME` regroupe tout dans un seul dossier (mode portable).

> **Lancé depuis l'application Claude (Windows)** : Claude est une application empaquetée (MSIX) et les
> programmes qu'elle lance voient `%APPDATA%` et `%LOCALAPPDATA%` redirigés vers
> `%LOCALAPPDATA%\Packages\Claude_…\LocalCache\`. Les données créées pendant le développement assisté
> s'y trouvent ; lancé normalement, Claudio utilise les vrais dossiers ci-dessus.
