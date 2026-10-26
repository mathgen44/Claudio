# Claudio

Assistant personnel pour Windows, **local d'abord** : il choisit, télécharge et fait
tourner des modèles de langage sur ta machine, en s'adaptant à sa puissance.

Voir [PLAN.md](PLAN.md) pour la vision complète et la feuille de route.

## État actuel (phases 0 et 1)

- Détection du matériel (processeur, mémoire, cartes graphiques via DXGI et NVML) et profil de machine S / M / L / XL.
- Catalogue de modèles GGUF (Qwen 3.5, Gemma 4, Qwen 3.6/3.8…) classés selon ce qui tient en mémoire.
- Téléchargement depuis Hugging Face avec reprise après coupure et vérification SHA-256.
- Installation automatique de llama.cpp (CUDA, Vulkan ou processeur selon la carte), chargement auto-ajusté à la mémoire libre.
- Réutilisation d'Ollama s'il est installé.
- Conversations en continu, historique SQLite, réflexion (« thinking ») affichable.
- Écrans : Mise en route, Conversations, Modèles, Paramètres. Jeton Hugging Face rangé dans le Gestionnaire d'identifiants Windows.

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

Autres commandes de l'outil : `status`, `runtime`, `install <id> <quant>`, `chat "<texte>"`.

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
| `crates/claudio-llm` | llama.cpp (installation, processus), Ollama, client de conversation compatible OpenAI |
| `crates/claudio-app` | Services de haut niveau partagés par l'application et l'outil en ligne de commande |
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
