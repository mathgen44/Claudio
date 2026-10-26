//! Plans : graphe d'étapes (DAG) produit par le planificateur, validé puis ordonné.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::defs::AgentSet;

pub const MAX_STEPS: usize = 8;
/// Clé de l'étape de synthèse ajoutée automatiquement.
pub const SYNTHESIS: &str = "synthese";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub instructions: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

/// Schéma JSON imposé au planificateur (sortie structurée).
pub fn schema(agents: &AgentSet) -> Value {
    let ids: Vec<&str> = agents.workers().map(|a| a.id.as_str()).collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["steps"],
        "properties": {
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "agent", "title", "instructions", "depends_on"],
                    "properties": {
                        "id": {"type": "string", "description": "identifiant court, ex. e1"},
                        "agent": {"type": "string", "enum": ids},
                        "title": {"type": "string"},
                        "instructions": {"type": "string"},
                        "depends_on": {"type": "array", "items": {"type": "string"}}
                    }
                }
            }
        }
    })
}

/// Consigne utilisateur du planificateur : objectif et agents disponibles.
pub fn planner_prompt(objective: &str, agents: &AgentSet) -> String {
    let list: Vec<String> = agents
        .workers()
        .map(|a| format!("- {} : {}", a.id, a.description))
        .collect();
    format!(
        "Objectif :\n{objective}\n\nAgents disponibles :\n{}\n\n\
         Réponds uniquement avec un objet JSON de la forme \
         {{\"steps\": [{{\"id\": \"e1\", \"agent\": \"...\", \"title\": \"...\", \"instructions\": \"...\", \"depends_on\": []}}]}}.",
        list.join("\n")
    )
}

/// Extrait le premier objet JSON d'un texte (le modèle peut l'entourer de prose).
fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// Lit et corrige le plan proposé : agents inconnus remplacés par l'assistant,
/// dépendances inconnues retirées, cycles refusés.
pub fn parse(text: &str, agents: &AgentSet) -> Result<Plan> {
    let v = extract_json(text).ok_or_else(|| anyhow::anyhow!("le plan n'est pas du JSON"))?;
    let mut plan: Plan = serde_json::from_value(v)?;
    if plan.steps.is_empty() {
        bail!("plan vide");
    }
    plan.steps.truncate(MAX_STEPS);
    let mut seen = HashSet::new();
    for (i, s) in plan.steps.iter_mut().enumerate() {
        if s.id.trim().is_empty() || s.id == SYNTHESIS || !seen.insert(s.id.clone()) {
            s.id = format!("e{}", i + 1);
            seen.insert(s.id.clone());
        }
        if agents.get(&s.agent).is_none() || s.agent == "planner" {
            s.agent = "assistant".into();
        }
        if s.title.trim().is_empty() {
            s.title = format!("Étape {}", i + 1);
        }
    }
    let ids: HashSet<String> = plan.steps.iter().map(|s| s.id.clone()).collect();
    for s in &mut plan.steps {
        let me = s.id.clone();
        s.depends_on.retain(|d| ids.contains(d) && *d != me);
        s.depends_on.dedup();
    }
    levels(&plan)?;
    Ok(plan)
}

/// Plan d'une seule étape confiée à un agent.
pub fn single(agent: &str, objective: &str) -> Plan {
    Plan {
        steps: vec![PlanStep {
            id: "e1".into(),
            agent: agent.into(),
            title: "Mission".into(),
            instructions: objective.into(),
            depends_on: vec![],
        }],
    }
}

/// Ajoute l'étape finale de vérification et de synthèse, qui dépend de toutes les autres.
pub fn with_synthesis(mut plan: Plan) -> Plan {
    if plan.steps.len() > 1 && !plan.steps.iter().any(|s| s.id == SYNTHESIS) {
        let all = plan.steps.iter().map(|s| s.id.clone()).collect();
        plan.steps.push(PlanStep {
            id: SYNTHESIS.into(),
            agent: "reviewer".into(),
            title: "Vérification et synthèse".into(),
            instructions: "Vérifie les résultats des étapes précédentes au regard de l'objectif, \
                signale les erreurs ou manques, puis rédige la réponse finale complète."
                .into(),
            depends_on: all,
        });
    }
    plan
}

/// Niveaux du graphe (tri topologique) : les étapes d'un même niveau sont
/// indépendantes entre elles. Erreur en cas de cycle.
pub fn levels(plan: &Plan) -> Result<Vec<Vec<String>>> {
    let mut remaining: HashMap<&str, HashSet<&str>> = plan
        .steps
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut out = Vec::new();
    while !remaining.is_empty() {
        let mut ready: Vec<&str> = remaining
            .iter()
            .filter(|(_, deps)| deps.is_empty())
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            bail!("le plan contient un cycle de dépendances");
        }
        ready.sort_by_key(|id| plan.steps.iter().position(|s| s.id == *id));
        for id in &ready {
            remaining.remove(id);
        }
        for deps in remaining.values_mut() {
            for id in &ready {
                deps.remove(id);
            }
        }
        out.push(ready.into_iter().map(str::to_owned).collect());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents() -> AgentSet {
        AgentSet::load(None)
    }

    #[test]
    fn parses_and_repairs_plans() {
        let text = r#"Voici le plan :
        {"steps": [
          {"id": "a", "agent": "researcher", "title": "Chercher", "instructions": "x", "depends_on": []},
          {"id": "b", "agent": "inconnu", "title": "", "instructions": "y", "depends_on": ["a", "zzz", "b"]},
          {"id": "a", "agent": "analyst", "title": "Doublon", "instructions": "z", "depends_on": ["a"]}
        ]}
        Bonne chance !"#;
        let p = parse(text, &agents()).unwrap();
        assert_eq!(p.steps[1].agent, "assistant");
        assert_eq!(p.steps[1].depends_on, vec!["a"]);
        assert_eq!(p.steps[1].title, "Étape 2");
        assert_eq!(p.steps[2].id, "e3", "identifiant en double renommé");
        assert_eq!(
            levels(&p).unwrap(),
            vec![
                vec!["a".to_string()],
                vec!["b".to_string(), "e3".to_string()]
            ]
        );
    }

    #[test]
    fn rejects_cycles_and_garbage() {
        let cyc = r#"{"steps": [
          {"id": "a", "agent": "coder", "title": "A", "instructions": "", "depends_on": ["b"]},
          {"id": "b", "agent": "coder", "title": "B", "instructions": "", "depends_on": ["a"]}]}"#;
        assert!(parse(cyc, &agents()).is_err());
        assert!(parse("pas de json", &agents()).is_err());
        assert!(parse(r#"{"steps": []}"#, &agents()).is_err());
    }

    #[test]
    fn synthesis_depends_on_everything() {
        let p = with_synthesis(Plan {
            steps: vec![
                single("researcher", "x").steps[0].clone(),
                PlanStep {
                    id: "e2".into(),
                    agent: "analyst".into(),
                    title: "B".into(),
                    instructions: "y".into(),
                    depends_on: vec![],
                },
            ],
        });
        let s = p.steps.last().unwrap();
        assert_eq!(s.id, SYNTHESIS);
        assert_eq!(s.depends_on, vec!["e1", "e2"]);
        assert_eq!(levels(&p).unwrap().len(), 2);
        // Une seule étape : pas de synthèse superflue.
        assert_eq!(with_synthesis(single("coder", "x")).steps.len(), 1);
    }

    #[test]
    fn schema_lists_worker_agents() {
        let s = schema(&agents());
        let ids = s
            .pointer("/properties/steps/items/properties/agent/enum")
            .unwrap();
        assert!(ids.as_array().unwrap().iter().any(|v| v == "researcher"));
        assert!(!ids.as_array().unwrap().iter().any(|v| v == "planner"));
    }
}
