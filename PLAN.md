# Claudio — Plan détaillé

> Assistant personnel complet pour Windows, local-first, multi-agents, capable de télécharger
> et d'exécuter ses propres modèles, avec un accès contrôlé aux ressources de la machine.

---

## 1. Vision et principes

| Principe | Traduction concrète |
|---|---|
| **Local d'abord** | Le routeur choisit un modèle local par défaut. Le cloud (API tokens) ne sert qu'en repli ou sur demande explicite, et uniquement pour les tâches qui l'exigent. |
| **S'adapte à la machine** | Au démarrage, Claudio profile le matériel (GPU/VRAM, RAM, CPU, disque) et en déduit un « profil » qui fixe les modèles, les quantifications, le contexte et le parallélisme. |
| **Autonome mais sous contrôle** | Les agents agissent seuls dans un périmètre de permissions explicite. Toute action hors périmètre déclenche une demande à l'utilisateur. |
| **Transparent** | Chaque action d'agent est journalisée, rejouable et visible dans l'interface (qui, quoi, quel modèle, quel coût, quels fichiers touchés). |
| **Extensible** | Outils via **MCP** (Model Context Protocol), compétences (« skills ») en fichiers Markdown, agents décrits en YAML. |

### Inspirations retenues

- **Cornelius** : séparation stricte local/cloud (perception et données restent sur la machine), mémoire par personne consultable/corrigeable/supprimable, page « Mise en route » (checklist de configuration), modules à contrat documenté, délégation des tâches longues à des agents.
- **OpenClaw** : passerelle (*gateway*) unique qui reçoit les messages de plusieurs canaux (UI, Telegram, etc.), sessions persistantes, compétences installables, tâches planifiées (cron), assistant « toujours allumé ».
- **Hermes Agent (Nous Research)** : boucle d'agent avec outils, mémoire long terme, **compétences auto-générées** (l'agent écrit une skill réutilisable après avoir résolu une tâche), sous-agents, support de nombreux fournisseurs de modèles.

---

## 2. Architecture générale

```
┌──────────────────────────────────────────────────────────────────────┐
│  UI (Tauri 2 + React/TypeScript)                                      │
│  Chat · Tableau de bord agents · Modèles · Permissions · Mémoire ·   │
│  Journal d'activité · Mise en route · Paramètres                      │
└───────────────▲──────────────────────────────────────────────────────┘
                │ IPC Tauri / WebSocket local (127.0.0.1, jeton)
┌───────────────┴──────────────────────────────────────────────────────┐
│  claudio-core (Rust, service/arrière-plan)                            │
│                                                                       │
│  ┌────────────┐  ┌──────────────┐  ┌───────────────┐  ┌────────────┐ │
│  │ Gateway    │  │ Orchestrateur│  │ Routeur de    │  │ Gestion-   │ │
│  │ sessions & │─▶│ (planner,    │─▶│ modèles       │─▶│ naire de   │ │
│  │ canaux     │  │ superviseur) │  │ local/cloud   │  │ runtimes   │ │
│  └────────────┘  └──────┬───────┘  └───────────────┘  └─────┬──────┘ │
│                         │                                    │        │
│  ┌────────────┐  ┌──────▼───────┐  ┌───────────────┐  ┌─────▼──────┐ │
│  │ Mémoire    │◀─│ Agents       │─▶│ Outils (MCP   │  │ llama.cpp  │ │
│  │ SQLite +   │  │ (boucle ReAct│  │ + natifs)     │  │ server(s)  │ │
│  │ vecteurs   │  │  + skills)   │  └──────┬────────┘  │ / Ollama   │ │
│  └────────────┘  └──────────────┘         │           └────────────┘ │
│                                    ┌──────▼────────┐  ┌────────────┐ │
│  ┌────────────┐  ┌──────────────┐  │ Moteur de     │  │ Hub HF     │ │
│  │ Profileur  │  │ Planificateur│  │ permissions   │  │ (recherche,│ │
│  │ matériel   │  │ (cron)       │  │ + sandbox     │  │ download)  │ │
│  └────────────┘  └──────────────┘  └───────────────┘  └────────────┘ │
│                                                                       │
│  Coffre de secrets : Windows Credential Manager (DPAPI)               │
└──────────────────────────────────────────────────────────────────────┘
        │ HTTPS uniquement si nécessaire
        ▼
  APIs distantes : Anthropic, OpenAI, Mistral, OpenRouter, Gemini, … + Hugging Face Hub
```

### Choix de la stack (proposition)

| Couche | Choix | Pourquoi |
|---|---|---|
| Application de bureau | **Tauri 2** | Binaire léger (~10 Mo vs ~150 Mo Electron), installeur MSI/NSIS natif, tray icon, auto-update, Rust côté système. |
| Cœur | **Rust** (tokio, axum, sqlx) | Performances, accès système Windows (WMI, DXGI, NVML), sécurité mémoire, un seul binaire. Rust 1.99 déjà installé. |
| Interface | **React + TypeScript + Vite + Tailwind + shadcn/ui** | Écosystème riche, rendu Markdown/code, streaming facile. Node 20 déjà installé. |
| Inférence locale | **llama.cpp `llama-server`** embarqué (builds CUDA / Vulkan / CPU) | Format GGUF = l'essentiel des modèles HF quantifiés, API compatible OpenAI, offload GPU partiel, très portable. **Ollama** supporté en option s'il est déjà installé (c'est le cas ici). |
| Embeddings | modèle GGUF d'embedding via llama.cpp (ex. `bge-m3`, `nomic-embed`) | Reste 100 % local. |
| Base de données | **SQLite** + extension **sqlite-vec** | Un fichier, zéro serveur, recherche vectorielle intégrée. |
| Outils | **MCP** (client intégré, serveurs stdio/HTTP) + outils natifs Rust | Standard ouvert, réutilise l'écosystème existant (filesystem, git, navigateur, etc.). |
| Voix (phase ultérieure) | whisper.cpp (STT), Piper/Kokoro (TTS), openWakeWord | Tout local, comme Cornelius. |

> Alternative évaluée : backend **Python** (FastAPI + transformers/vLLM). Plus d'écosystème IA, mais
> distribution Windows lourde (environnement Python embarqué, ~1–3 Go), démarrage lent et gestion
> CUDA fragile. Je recommande Rust + llama.cpp ; Python peut rester disponible comme *outil* sandboxé
> (exécution de scripts) sans être le cœur.

---

## 3. Modules en détail

### 3.1 Profileur matériel → « profil de machine »

Détection au premier lancement puis à chaque démarrage (rapide) :

- GPU : fabricant, modèle, VRAM totale/libre (NVML pour NVIDIA, DXGI pour AMD/Intel), version CUDA/driver, support Vulkan.
- CPU : cœurs physiques/logiques, AVX2/AVX-512.
- RAM totale/libre, espace disque du dossier de modèles.
- Mini-benchmark optionnel (tokens/s sur un petit modèle) pour calibrer.

Le profil fixe :

| Profil | Exemple | Modèle principal | Modèles secondaires | Parallélisme |
|---|---|---|---|---|
| **S** — CPU / iGPU, ≤ 16 Go RAM | portable bureautique | 1–4 B (Q4) | embedding seul | 1 agent actif |
| **M** — GPU 6–8 Go | RTX 3060 / 4060 | 7–9 B (Q4_K_M) | petit 1–3 B pour le routage | 2 agents |
| **L** — GPU 12–16 Go | **RTX 5070 Ti 16 Go (ta machine)** | 14–24 B (Q4/Q5) ou MoE 30B-A3B | 3–4 B rapide + vision | 3–4 agents |
| **XL** — GPU ≥ 24 Go / multi-GPU | RTX 4090/5090 | 32 B+ / 70 B partiellement offloadé | plusieurs spécialistes | 4+ agents |

Règle de dimensionnement : `VRAM nécessaire ≈ taille du fichier GGUF + cache KV(contexte) + marge 10 %`.
Le gestionnaire de runtimes choisit la quantification et la longueur de contexte qui tiennent, sinon
répartit les couches entre GPU et CPU, sinon propose un modèle plus petit.

### 3.2 Hub de modèles (Hugging Face)

- Recherche via l'API HF (`/api/models?filter=gguf&search=…`), lecture des fichiers d'un dépôt, des licences et des tailles.
- **Catalogue recommandé** intégré (JSON mis à jour) : pour chaque *rôle* (général, code, raisonnement, vision, embedding, petit routeur) et chaque profil, le meilleur dépôt/quantification connu.
- Téléchargement : reprise sur interruption, téléchargements parallèles par segments, vérification SHA-256, support du jeton HF pour les modèles à accès restreint, quota disque configurable.
- **Autonomie** : un agent peut proposer/déclencher le téléchargement d'un modèle si une tâche l'exige (ex. besoin de vision). Politique configurable : *jamais / demander / automatique sous X Go*.
- Registre local : modèles installés, rôle, profil compatible, perfs mesurées, dernière utilisation ; nettoyage des modèles inutilisés.

### 3.3 Gestionnaire de runtimes

- Lance/arrête des processus `llama-server` (un par modèle chargé), ports dynamiques sur 127.0.0.1.
- Chargement à la demande, déchargement LRU quand la VRAM manque, préchauffage du modèle principal.
- Choix automatique du build (CUDA si NVIDIA compatible, sinon Vulkan, sinon CPU).
- Adaptateur **Ollama** si installé (réutilise ses modèles déjà téléchargés).
- Interface unifiée `LlmProvider` (chat, streaming, tool-calling, embeddings) partagée avec les fournisseurs cloud.

### 3.4 Fournisseurs distants

- Anthropic, OpenAI, Mistral, Google Gemini, OpenRouter, + tout endpoint compatible OpenAI (LM Studio, vLLM distant, serveur maison).
- Clés stockées dans **Windows Credential Manager**, jamais en clair sur disque ni dans les logs.
- Suivi de consommation (tokens, coût estimé) par agent/tâche, **budgets** journaliers/mensuels avec blocage.
- « Agents distants » = un agent dont le modèle est cloud, **ou** une instance Claudio sur une autre machine (phase 6 : nœuds distants via connexion chiffrée).

### 3.5 Routeur de modèles

Pour chaque appel, score = f(capacité requise, confidentialité, latence, coût) :

1. Les données sensibles (fichiers locaux marqués privés, mémoire personnelle) → **local obligatoire**.
2. Tâche simple (classification, résumé court, routage) → petit modèle local rapide.
3. Tâche standard → modèle principal local.
4. Tâche dure (échec local, contexte énorme, raisonnement long) → escalade cloud **si autorisée** par la politique, avec indication dans l'UI.
5. Repli automatique si un fournisseur est indisponible.

### 3.6 Orchestrateur multi-agents

- **Agents déclaratifs** (YAML) : nom, rôle, prompt système, modèle/rôle de modèle préféré, outils autorisés, permissions, budget.
  Agents fournis : `assistant` (généraliste, point d'entrée), `planner`, `researcher` (web), `coder`, `files` (gestion de fichiers), `analyst` (données/tableurs), `reviewer` (vérification).
- **Flux** : l'assistant reçoit la demande → le *planner* la décompose en graphe de tâches (DAG) → le superviseur affecte chaque tâche à l'agent adapté → exécution parallèle dans la limite du profil → le *reviewer* valide → synthèse.
- Boucle d'agent : ReAct / tool-calling natif, limite d'itérations, détection de boucle, reprise après crash (état persisté en SQLite).
- **Tâches longues en arrière-plan** avec notifications Windows, et **tâches planifiées** (cron) à la OpenClaw.
- **Skills auto-apprises** à la Hermes : après une tâche réussie, l'agent peut rédiger une skill Markdown (procédure réutilisable) soumise à validation de l'utilisateur.

### 3.7 Outils

Natifs : système de fichiers (lecture/écriture/recherche), exécution de commandes (PowerShell, sandboxée), recherche web + lecture de pages, presse-papiers, capture d'écran + vision, notifications, horloge/calendrier local, lecture PDF/Office.
Via MCP : n'importe quel serveur MCP (GitHub, navigateur, bases de données, domotique…), avec un gestionnaire dans l'UI.

### 3.8 Permissions et sécurité (point critique)

- **Accès sur demande** : par défaut, aucun répertoire n'est accessible. L'utilisateur accorde des « portées » :
  `D:\Documents\Factures` → lecture seule · `D:\GIT\MonProjet` → lecture/écriture · réseau → oui/non · commandes shell → demander à chaque fois.
- Durée : *une fois*, *cette session*, *permanente* ; révocable à tout moment.
- Toute action hors portée → **boîte de dialogue** (qui demande, pourquoi, quoi exactement).
- Actions destructrices (suppression, écrasement, envoi externe) → confirmation systématique + corbeille/sauvegarde avant modification.
- Exécution de code dans un **sandbox** (Windows Job Objects + AppContainer / jeton restreint, sans accès réseau par défaut).
- Défense contre l'injection de prompt : le contenu lu (pages web, fichiers) est marqué « non fiable » et ne peut pas élargir les permissions.
- Journal d'audit immuable de chaque appel d'outil.

### 3.9 Mémoire

- **Court terme** : historique de session, résumé glissant quand le contexte sature.
- **Long terme** : faits et préférences extraits des conversations (SQLite + vecteurs), visibles, éditables, supprimables (à la Cornelius).
- **Connaissances (RAG)** : indexation des répertoires autorisés (texte, PDF, Office, code), mise à jour incrémentale via surveillance de fichiers.
- **Mémoire procédurale** : les skills.

### 3.10 Interface utilisateur

- Fenêtre principale + **icône de zone de notification** + **raccourci global** (ex. `Ctrl+Espace`) pour une barre de commande rapide.
- Écrans : Chat (streaming, pièces jointes, glisser-déposer), Agents (graphe des tâches en cours, logs en direct, arrêt), Modèles (installés, catalogue, téléchargements, VRAM), Permissions, Mémoire, Journal, Fournisseurs & budgets, **Mise en route** (checklist), Paramètres.
- Thème clair/sombre, interface en français (i18n prête pour l'anglais).

### 3.11 Canaux (optionnel, inspiré d'OpenClaw)

Telegram / Discord / e-mail pour parler à Claudio à distance, via la même gateway, avec une liste blanche d'expéditeurs.

---

## 4. Arborescence prévue du dépôt

```
Claudio/
├─ apps/
│  └─ desktop/            # Tauri 2 : src-tauri (Rust) + src (React)
├─ crates/
│  ├─ claudio-core/       # orchestrateur, gateway, sessions
│  ├─ claudio-agents/     # boucle d'agent, définitions, skills
│  ├─ claudio-llm/        # trait LlmProvider, local (llama.cpp/Ollama) + cloud
│  ├─ claudio-models/     # hub HF, téléchargement, registre, catalogue
│  ├─ claudio-hw/         # profileur matériel
│  ├─ claudio-tools/      # outils natifs + client MCP
│  ├─ claudio-perms/      # moteur de permissions, sandbox, audit
│  └─ claudio-memory/     # SQLite, sqlite-vec, RAG
├─ agents/                # définitions YAML des agents fournis
├─ skills/                # skills Markdown fournies
├─ catalog/models.json    # catalogue de modèles recommandés par profil
├─ docs/
└─ PLAN.md
```

Données utilisateur : `%APPDATA%\Claudio\` (config, base, logs) et dossier de modèles configurable (par défaut `%LOCALAPPDATA%\Claudio\models`, déplaçable sur un autre disque).

---

## 5. Feuille de route par phases

Chaque phase se termine par une version **utilisable** et testée.

| Phase | Contenu | Livrable |
|---|---|---|
| **0 — Fondations** (≈1 sem.) | Dépôt git, workspace Cargo, squelette Tauri+React, CI GitHub Actions, logs, config, stockage des secrets. | Fenêtre vide qui démarre, installeur de test. |
| **1 — Chat local** (≈2 sem.) | Profileur matériel, intégration llama-server (CUDA/Vulkan/CPU), adaptateur Ollama, téléchargement HF d'un modèle depuis le catalogue, chat en streaming, historique SQLite. | **MVP** : discuter avec un modèle local choisi automatiquement selon la machine. |
| **2 — Hub de modèles & cloud** (≈2 sem.) | Recherche HF complète, gestion des téléchargements, registre, LRU VRAM ; fournisseurs cloud + routeur + budgets. | Choix libre de modèles, bascule local/cloud. |
| **3 — Outils & permissions** (≈2–3 sem.) | Moteur de permissions, portées de répertoires, dialogues, audit, outils fichiers/web/shell sandboxé, client MCP. | Un agent unique capable d'agir sur des dossiers autorisés. |
| **4 — Multi-agents** (≈3 sem.) | Agents YAML, planner/superviseur/DAG, exécution parallèle adaptée au profil, tableau de bord, tâches de fond, cron. | Orchestration complète. |
| **5 — Mémoire & RAG** (≈2 sem.) | Mémoire long terme éditable, indexation des dossiers, skills auto-apprises. | Claudio se souvient et apprend. |
| **6 — Extensions** (continu) | Voix (wake word, STT, TTS), canaux (Telegram…), nœuds Claudio distants, vision écran, auto-update, signature de code. | Assistant « complet ». |

### Critères de qualité transverses
- Tests unitaires Rust + tests d'intégration avec un petit modèle GGUF (≈100 Mo) en CI.
- Tests E2E de l'UI (Playwright).
- Aucun secret dans les logs ; revue sécurité à la fin des phases 3 et 4.
- Mesures de perf : temps jusqu'au premier token, tokens/s, VRAM, affichées dans l'UI.

---

## 6. Risques et parades

| Risque | Parade |
|---|---|
| Diversité des GPU/drivers Windows | 3 builds llama.cpp + détection + repli CPU ; Ollama en alternative. |
| Petits modèles locaux moins fiables en *tool-calling* | Modèles choisis pour leur support des outils, grammaires JSON (GBNF) pour forcer un format valide, escalade cloud optionnelle. |
| Sécurité (agent qui agit sur le PC) | Refus par défaut, portées explicites, sandbox, confirmation des actions destructrices, audit. |
| Taille des modèles / disque | Quotas, nettoyage, dossier déplaçable, avertissement avant gros téléchargement. |
| Licences des modèles | Affichage et acceptation de la licence avant téléchargement ; filtrage possible (ex. usage commercial). |
| Antivirus / SmartScreen | Signature de code (phase 6), binaires llama.cpp officiels vérifiés par hash. |

---

## 7. Environnement constaté sur ta machine

- GPU **NVIDIA RTX 5070 Ti, 16 Go** → profil **L**.
- Rust 1.99, .NET 10, Node 20.19, Python 3.10, Ollama 0.13 (installé, non démarré).
- Le dossier `D:\GIT\Claudio` est vide et n'est pas encore un dépôt git.

---

## 8. Décisions validées (2026-10-26)

| Sujet | Décision |
|---|---|
| Stack | **Tauri 2 + Rust** (cœur), React/TypeScript (UI) |
| Inférence locale | **llama.cpp embarqué**, Ollama réutilisé s'il est présent |
| Premier livrable | **Phases 0 et 1** : appli qui profile la machine, télécharge un modèle HF adapté et chatte en local |
| Code source | git local + **dépôt GitHub privé** |

Restent ouverts (à trancher plus tard) : licence du projet, canaux distants, voix.

---

## 9. Avancement

### Phases 0 et 1 — livrées (2026-10-26)

- Workspace Cargo (`claudio-core`, `-hw`, `-models`, `-llm`, `-app`) + application Tauri 2, CI GitHub Actions.
- Journaux quotidiens, configuration TOML, secrets dans le Gestionnaire d'identifiants Windows, SQLite avec migrations.
- Profileur matériel (DXGI + NVML) et profils S/M/L/XL ; catalogue de 8 modèles GGUF avec sélection par mémoire.
- Téléchargements Hugging Face avec reprise et vérification SHA-256 ; installation de llama.cpp officiel (CUDA/Vulkan/CPU, empreintes vérifiées).
- llama-server piloté par Claudio (job Windows : arrêt garanti avec l'application, `--fit` pour répartir GPU/CPU), Ollama en option.
- Interface : Mise en route, Conversations (flux continu, réflexion repliable), Modèles, Paramètres.

Mesuré sur la machine de développement (RTX 5070 Ti) : ~440 jetons/s avec Qwen 3.5 0.8B.

### Ajustements par rapport au plan initial

- Les tailles de cache KV du catalogue sont des estimations prudentes ; llama.cpp ajuste lui-même le placement (`--fit`), ce qui rend aussi utilisables les gros modèles MoE en mode hybride GPU+RAM.
- La réflexion est coupée par défaut sur les modèles de moins de 9 milliards de paramètres (ils tournent en rond).

### Phase 2 — livrée (2026-10-26)

- Hub Hugging Face dans l'interface : recherche, ouverture d'un dépôt par son nom, variantes GGUF (fichiers découpés réunis), estimation de la mémoire et du placement à partir des métadonnées GGUF, licence à accepter avant téléchargement si elle n'est pas libre, dépôts restreints signalés.
- Placement « hybride » GPU + RAM pour les mélanges d'experts (catalogue et dépôts quelconques).
- Fournisseurs distants : Anthropic natif (`anthropic-version: 2026-10-21`, réflexion adaptative résumée, repli serveur en cas de refus), OpenAI, Mistral, Gemini, OpenRouter (prix récupérés), API compatibles OpenAI (adresse https obligatoire sauf localhost). Clés dans le Gestionnaire d'identifiants.
- Routeur pur et testé : modes local / automatique / cloud, réglage par conversation, escalade vers le cloud quand la conversation dépasse le contexte local, repli quand la cible échoue avant d'avoir répondu, historique raccourci pour le modèle local.
- Comptabilité : table `usage`, coût par message, consommation du mois par modèle, dépense sur 30 jours, plafonds journalier et mensuel (blocage à 100 %, alerte à 80 %).
- Le raisonnement et la décision du routeur sont conservés dans l'historique.

### Reporté

- Gestion de plusieurs modèles locaux chargés en même temps (LRU sur la mémoire vidéo) : utile surtout avec les agents parallèles, prévue avec la phase 4.
- Téléchargements segmentés en parallèle.
- Icône de zone de notification, raccourci global.
