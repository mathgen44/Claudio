//! Routeur : décide qui répond, du modèle local ou du modèle distant.
//!
//! La logique est pure (aucun accès réseau ni disque) pour être testée à part.

use anyhow::{Result, bail};
use claudio_core::config::RoutingMode;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Local,
    Cloud,
}

#[derive(Debug, Clone)]
pub struct RouteInput {
    pub mode: RoutingMode,
    /// Un modèle local (llama.cpp ou Ollama) est sélectionné.
    pub local_ready: bool,
    /// Contexte du modèle local, en jetons.
    pub local_context: u32,
    /// Taille estimée de la conversation, en jetons.
    pub prompt_tokens: u32,
    /// Un modèle distant est configuré, activé et a sa clé.
    pub cloud_ready: bool,
    /// Le budget permet encore une demande distante.
    pub budget_ok: bool,
    /// La conversation contient des images.
    pub has_images: bool,
    /// Le modèle local voit les images (projecteur de vision installé).
    pub local_vision: bool,
    /// Le modèle distant voit les images.
    pub cloud_vision: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub target: Target,
    pub reason: String,
    /// Cible de secours si la première échoue avant d'avoir répondu.
    pub fallback: Option<Target>,
}

/// Part du contexte local réservée à la réponse.
pub fn local_prompt_budget(context: u32) -> u32 {
    context.saturating_sub((context / 4).max(1024))
}

pub fn decide(i: &RouteInput) -> Result<Decision> {
    let cloud_ok = i.cloud_ready && i.budget_ok;
    let fits_locally = i.prompt_tokens <= local_prompt_budget(i.local_context);
    let decision = |target, reason: &str, fallback| Decision {
        target,
        reason: reason.to_owned(),
        fallback,
    };
    Ok(match i.mode {
        RoutingMode::Local => {
            if !i.local_ready {
                bail!("mode local : aucun modèle local n'est sélectionné");
            }
            decision(Target::Local, "mode local", None)
        }
        RoutingMode::Auto => {
            // Une image que le modèle local ne verrait pas part vers un modèle
            // distant qui la voit (sinon, seul son texte reconnu serait lu).
            if i.has_images && !i.local_vision && i.cloud_vision && cloud_ok {
                let reason = if i.local_ready {
                    "image : le modèle local ne voit pas les images"
                } else {
                    "aucun modèle local disponible"
                };
                return Ok(decision(
                    Target::Cloud,
                    reason,
                    i.local_ready.then_some(Target::Local),
                ));
            }
            if i.local_ready && (fits_locally || !cloud_ok) {
                let reason = if fits_locally {
                    "modèle local"
                } else if !i.cloud_ready {
                    "modèle local (début de conversation résumé : pas de modèle distant)"
                } else {
                    "modèle local (début de conversation coupé : budget cloud atteint)"
                };
                decision(Target::Local, reason, cloud_ok.then_some(Target::Cloud))
            } else if cloud_ok {
                let reason = if i.local_ready {
                    "conversation trop longue pour le modèle local"
                } else {
                    "aucun modèle local disponible"
                };
                decision(Target::Cloud, reason, None)
            } else if i.cloud_ready {
                bail!("budget cloud atteint et aucun modèle local disponible")
            } else {
                bail!(
                    "aucun modèle disponible : installez un modèle local ou configurez un fournisseur"
                )
            }
        }
        RoutingMode::Cloud => {
            if cloud_ok {
                decision(
                    Target::Cloud,
                    "mode cloud",
                    i.local_ready.then_some(Target::Local),
                )
            } else if i.local_ready {
                let reason = if i.cloud_ready {
                    "repli local : budget cloud atteint"
                } else {
                    "repli local : aucun modèle distant configuré"
                };
                decision(Target::Local, reason, None)
            } else if i.cloud_ready {
                bail!("budget cloud atteint")
            } else {
                bail!("mode cloud : aucun modèle distant n'est configuré")
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(mode: RoutingMode) -> RouteInput {
        RouteInput {
            mode,
            local_ready: true,
            local_context: 8192,
            prompt_tokens: 500,
            cloud_ready: true,
            budget_ok: true,
            has_images: false,
            local_vision: false,
            cloud_vision: true,
        }
    }

    #[test]
    fn auto_sends_images_where_they_are_seen() {
        let mut i = input(RoutingMode::Auto);
        i.has_images = true;
        let d = decide(&i).unwrap();
        assert_eq!((d.target, d.fallback), (Target::Cloud, Some(Target::Local)));
        assert!(d.reason.contains("image"));
        // Le modèle local voit : il reste prioritaire.
        i.local_vision = true;
        assert_eq!(decide(&i).unwrap().target, Target::Local);
        // Ni l'un ni l'autre ne voit : local, avec le texte reconnu.
        i.local_vision = false;
        i.cloud_vision = false;
        assert_eq!(decide(&i).unwrap().target, Target::Local);
        // Le mode local ne quitte jamais la machine, même pour une image.
        let mut l = input(RoutingMode::Local);
        l.has_images = true;
        assert_eq!(decide(&l).unwrap().target, Target::Local);
    }

    #[test]
    fn auto_prefers_local_with_cloud_fallback() {
        let d = decide(&input(RoutingMode::Auto)).unwrap();
        assert_eq!(d.target, Target::Local);
        assert_eq!(d.fallback, Some(Target::Cloud));
    }

    #[test]
    fn auto_escalates_long_conversations() {
        let mut i = input(RoutingMode::Auto);
        i.prompt_tokens = 7000;
        assert_eq!(decide(&i).unwrap().target, Target::Cloud);
        // Sans budget, on reste local et la conversation sera raccourcie.
        i.budget_ok = false;
        let d = decide(&i).unwrap();
        assert_eq!(d.target, Target::Local);
        assert_eq!(d.fallback, None);
        assert!(d.reason.contains("budget"));
    }

    #[test]
    fn auto_without_local_uses_cloud() {
        let mut i = input(RoutingMode::Auto);
        i.local_ready = false;
        assert_eq!(decide(&i).unwrap().target, Target::Cloud);
        i.budget_ok = false;
        assert!(decide(&i).is_err());
    }

    #[test]
    fn local_mode_never_leaves_the_machine() {
        let mut i = input(RoutingMode::Local);
        i.prompt_tokens = 100_000;
        let d = decide(&i).unwrap();
        assert_eq!((d.target, d.fallback), (Target::Local, None));
        i.local_ready = false;
        assert!(decide(&i).is_err());
    }

    #[test]
    fn cloud_mode_falls_back_to_local() {
        let d = decide(&input(RoutingMode::Cloud)).unwrap();
        assert_eq!((d.target, d.fallback), (Target::Cloud, Some(Target::Local)));
        let mut i = input(RoutingMode::Cloud);
        i.cloud_ready = false;
        assert_eq!(decide(&i).unwrap().target, Target::Local);
    }

    #[test]
    fn prompt_budget() {
        assert_eq!(local_prompt_budget(8192), 6144);
        assert_eq!(local_prompt_budget(2048), 1024);
    }
}
