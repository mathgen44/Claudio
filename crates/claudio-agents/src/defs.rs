//! Définitions d'agents (YAML) : fournies avec Claudio ou ajoutées par l'utilisateur.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Agents fournis, embarqués depuis le dossier `agents/` du dépôt.
const BUILTIN: &[(&str, &str)] = &[
    ("assistant", include_str!("../../../agents/assistant.yaml")),
    ("planner", include_str!("../../../agents/planner.yaml")),
    (
        "researcher",
        include_str!("../../../agents/researcher.yaml"),
    ),
    ("coder", include_str!("../../../agents/coder.yaml")),
    ("files", include_str!("../../../agents/files.yaml")),
    ("analyst", include_str!("../../../agents/analyst.yaml")),
    ("reviewer", include_str!("../../../agents/reviewer.yaml")),
];

/// Où l'agent préfère s'exécuter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelPref {
    /// Suit le réglage général de routage.
    #[default]
    Auto,
    Local,
    Cloud,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    /// Sert au planificateur pour choisir l'agent : à quoi il est bon.
    pub description: String,
    #[serde(default)]
    pub model: ModelPref,
    /// Outils autorisés : noms exacts ou motifs terminés par `*` (`web_*`, `mcp__*`, `*`).
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_steps")]
    pub max_steps: usize,
    /// Forcer la réflexion (sinon réglage général).
    #[serde(default)]
    pub thinking: Option<bool>,
    pub system_prompt: String,
    /// Agent fourni par Claudio (calculé, non lu dans le fichier).
    #[serde(skip_deserializing, default)]
    pub builtin: bool,
    /// Fichier du dossier utilisateur qui définit l'agent (calculé).
    #[serde(skip_deserializing, default)]
    pub file: Option<String>,
    /// Ce fichier remplace un agent fourni (calculé).
    #[serde(skip_deserializing, default)]
    pub overrides_builtin: bool,
}

fn default_steps() -> usize {
    12
}

impl AgentDef {
    pub fn parse(text: &str) -> Result<Self> {
        let a: AgentDef = serde_yaml_ng::from_str(text).context("fichier d'agent invalide")?;
        a.validate()?;
        Ok(a)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("l'agent {} n'a pas de nom", self.id);
        }
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!(
                "identifiant d'agent invalide : {:?} (lettres, chiffres, - et _)",
                self.id
            );
        }
        if self.system_prompt.trim().is_empty() {
            bail!("l'agent {} n'a pas de consignes (system_prompt)", self.id);
        }
        if self.max_steps == 0 || self.max_steps > 50 {
            bail!("max_steps doit être entre 1 et 50");
        }
        Ok(())
    }

    /// Texte YAML de l'agent, tel qu'enregistré dans le dossier utilisateur.
    pub fn to_yaml(&self) -> Result<String> {
        let mut v = serde_json::to_value(self)?;
        let obj = v.as_object_mut().expect("objet");
        for computed in ["builtin", "file", "overrides_builtin"] {
            obj.remove(computed);
        }
        if obj.get("thinking").is_some_and(|t| t.is_null()) {
            obj.remove("thinking");
        }
        Ok(serde_yaml_ng::to_string(&v)?)
    }

    /// L'agent a-t-il droit à cet outil ?
    pub fn allows_tool(&self, tool: &str) -> bool {
        self.tools.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => tool.starts_with(prefix),
            None => p == tool,
        })
    }
}

/// Ensemble des agents disponibles.
#[derive(Debug, Clone)]
pub struct AgentSet {
    pub agents: Vec<AgentDef>,
    /// Fichiers ignorés car invalides, avec la raison.
    pub errors: Vec<(String, String)>,
}

impl AgentSet {
    /// Agents fournis, puis ceux du dossier utilisateur (un fichier peut remplacer un
    /// agent fourni en reprenant son identifiant).
    pub fn load(user_dir: Option<&Path>) -> Self {
        let mut agents: Vec<AgentDef> = BUILTIN
            .iter()
            .map(|(id, text)| {
                let mut a =
                    AgentDef::parse(text).unwrap_or_else(|e| panic!("agent fourni {id} : {e:#}"));
                a.builtin = true;
                a
            })
            .collect();
        let mut errors = Vec::new();
        if let Some(dir) = user_dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            let mut files: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
                .collect();
            files.sort();
            for path in files {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| AgentDef::parse(&t))
                {
                    Ok(mut a) => {
                        a.overrides_builtin = BUILTIN.iter().any(|(id, _)| *id == a.id);
                        a.file = Some(name);
                        agents.retain(|x| x.id != a.id);
                        agents.push(a);
                    }
                    Err(e) => {
                        tracing::warn!("agent {name} ignoré : {e:#}");
                        errors.push((name, format!("{e:#}")));
                    }
                }
            }
        }
        Self { agents, errors }
    }

    pub fn get(&self, id: &str) -> Option<&AgentDef> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Enregistre un agent dans le dossier utilisateur : dans son fichier s'il en a
    /// un, sinon dans `<id>.yaml`. Un agent fourni ainsi modifié est remplacé.
    /// `create` : nouvel agent, dont l'identifiant doit être libre.
    pub fn save(&self, dir: &Path, agent: &AgentDef, create: bool) -> Result<()> {
        agent.validate()?;
        let existing = self.get(&agent.id);
        if create && existing.is_some() {
            bail!("un agent porte déjà l'identifiant « {} »", agent.id);
        }
        let file = existing
            .and_then(|a| a.file.clone())
            .unwrap_or_else(|| format!("{}.yaml", agent.id));
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(file), agent.to_yaml()?)?;
        Ok(())
    }

    /// Supprime le fichier d'un agent : un agent personnalisé disparaît, un agent
    /// fourni retrouve sa définition d'origine.
    pub fn remove(&self, dir: &Path, id: &str) -> Result<()> {
        let agent = self.get(id).context("agent introuvable")?;
        let file = agent
            .file
            .as_ref()
            .context("agent fourni par Claudio : il n'a pas de fichier à supprimer")?;
        std::fs::remove_file(dir.join(file))?;
        Ok(())
    }

    /// Agents pouvant recevoir une étape (le planificateur ne s'attribue pas d'étape).
    pub fn workers(&self) -> impl Iterator<Item = &AgentDef> {
        self.agents.iter().filter(|a| a.id != "planner")
    }
}

/// Texte d'aide écrit dans le dossier des agents de l'utilisateur.
pub const README: &str = "Agents personnalisés de Claudio
================================

Chaque fichier .yaml de ce dossier définit un agent. Un fichier qui reprend
l'identifiant d'un agent fourni (assistant, researcher, coder, files, analyst,
reviewer, planner) le remplace. Exemple : traducteur.yaml

id: traducteur
name: Traducteur
description: Traduit des documents entre le français et l'anglais.
model: auto            # auto, local ou cloud
tools: [\"read_file\", \"write_file\", \"list_directory\"]
max_steps: 8
system_prompt: |
  Tu es un traducteur professionnel. Tu respectes le ton et la mise en forme.

Outils : list_directory, read_file, search_files, write_file, create_directory,
move_path, delete_path, web_fetch, web_search, run_powershell, mcp__<serveur>__<outil>.
Un motif terminé par * couvre plusieurs outils (web_*, mcp__*, * pour tous).
Les outils restent soumis aux autorisations de la page « Outils et accès ».
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_valid() {
        let set = AgentSet::load(None);
        assert_eq!(set.agents.len(), BUILTIN.len());
        assert!(set.get("planner").unwrap().tools.is_empty());
        assert!(!set.workers().any(|a| a.id == "planner"));
        let researcher = set.get("researcher").unwrap();
        assert!(researcher.allows_tool("web_search"));
        assert!(!researcher.allows_tool("run_powershell"));
        assert!(set.get("assistant").unwrap().allows_tool("mcp__gh__search"));
    }

    #[test]
    fn user_agents_override_and_errors_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("traducteur.yaml"),
            "id: traducteur\nname: Traducteur\ndescription: Traduit.\ntools: [\"read_file\", \"web_*\"]\nsystem_prompt: Tu traduis.\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("coder.yaml"),
            "id: coder\nname: Mon dev\ndescription: Perso.\nsystem_prompt: Code.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("casse.yaml"), "id: [pas une chaîne\n").unwrap();
        std::fs::write(
            dir.path().join("inconnu.yaml"),
            "id: x\nname: X\ndescription: d\nsystem_prompt: s\nfoo: 1\n",
        )
        .unwrap();
        let set = AgentSet::load(Some(dir.path()));
        let t = set.get("traducteur").unwrap();
        assert!(t.allows_tool("web_fetch") && !t.allows_tool("write_file"));
        assert!(!t.builtin);
        assert_eq!(set.get("coder").unwrap().name, "Mon dev");
        assert_eq!(set.errors.len(), 2);
        assert_eq!(t.file.as_deref(), Some("traducteur.yaml"));
        assert!(set.get("coder").unwrap().overrides_builtin && !t.overrides_builtin);
    }

    #[test]
    fn save_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let set = AgentSet::load(Some(dir.path()));
        // Agent fourni modifié : un fichier le remplace.
        let mut coder = set.get("coder").unwrap().clone();
        coder.name = "Développeur maison".into();
        coder.thinking = Some(true);
        set.save(dir.path(), &coder, false).unwrap();
        let mut new = coder.clone();
        new.id = "traducteur".into();
        new.system_prompt = "Ligne 1
Ligne « 2 » : guillemets \"ok\"
"
        .into();
        set.save(dir.path(), &new, true).unwrap();
        assert!(set.save(dir.path(), &coder, true).is_err());

        let set = AgentSet::load(Some(dir.path()));
        assert!(set.errors.is_empty(), "{:?}", set.errors);
        let c = set.get("coder").unwrap();
        assert_eq!(
            (c.name.as_str(), c.thinking),
            ("Développeur maison", Some(true))
        );
        assert!(c.overrides_builtin);
        let t = set.get("traducteur").unwrap();
        assert_eq!(t.system_prompt, new.system_prompt);
        assert_eq!(t.tools, coder.tools);

        set.remove(dir.path(), "coder").unwrap();
        set.remove(dir.path(), "traducteur").unwrap();
        let set = AgentSet::load(Some(dir.path()));
        assert_eq!(
            set.get("coder").unwrap().name,
            AgentSet::load(None).get("coder").unwrap().name
        );
        assert!(set.get("traducteur").is_none());
        assert!(set.remove(dir.path(), "coder").is_err());
    }
}
