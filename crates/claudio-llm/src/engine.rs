//! Modèles locaux chargés : un serveur llama.cpp par modèle, autant qu'il en
//! tient dans la mémoire. Quand la place manque, le modèle inutilisé depuis le
//! plus longtemps est déchargé ; un modèle en cours d'utilisation ne l'est jamais.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::Mutex;

use crate::chat::Endpoint;
use crate::server::{LlamaServer, ServerConfig};

/// Attente maximale d'une place libre (les autres modèles finissent leurs demandes).
const WAIT_FOR_ROOM: Duration = Duration::from_secs(20 * 60);

struct Slot {
    server: LlamaServer,
    /// Partagé avec chaque point d'accès distribué : le modèle est occupé tant
    /// qu'il en reste un en circulation.
    lease: Arc<()>,
    last_used: Instant,
}

impl Slot {
    fn idle(&self) -> bool {
        Arc::strong_count(&self.lease) == 1
    }

    fn endpoint(&mut self) -> Endpoint {
        self.last_used = Instant::now();
        Endpoint {
            lease: Some(self.lease.clone()),
            ..self.server.endpoint()
        }
    }
}

#[derive(Default)]
pub struct LocalEngine {
    slots: Mutex<Vec<Slot>>,
}

fn same(a: &ServerConfig, b: &ServerConfig) -> bool {
    a.model_path == b.model_path
        && a.context == b.context
        && a.use_gpu == b.use_gpu
        && a.parallel == b.parallel
        && a.embedding_pooling == b.embedding_pooling
        && a.exe == b.exe
}

impl LocalEngine {
    /// Renvoie le point d'accès du modèle demandé, en le chargeant si besoin.
    /// `budget` : mémoire disponible pour l'ensemble des modèles chargés.
    /// Le modèle reste chargé tant que le point d'accès renvoyé (ou une copie) existe.
    pub async fn ensure(
        &self,
        http: &reqwest::Client,
        config: ServerConfig,
        budget: u64,
    ) -> Result<Endpoint> {
        let started = Instant::now();
        loop {
            {
                let mut slots = self.slots.lock().await;
                slots.retain_mut(|s| s.server.is_running());
                if let Some(s) = slots.iter_mut().find(|s| same(&s.server.config, &config)) {
                    return Ok(s.endpoint());
                }
                // Même modèle avec d'autres réglages : remplacé dès qu'il est libre.
                let stale = |s: &Slot| s.server.config.model_path == config.model_path;
                if !slots.iter().any(|s| stale(s) && !s.idle()) {
                    for s in extract(&mut slots, |s| stale(s)) {
                        s.server.stop().await;
                    }
                    if let Some(evicted) = make_room(&mut slots, config.memory, budget) {
                        for s in evicted {
                            tracing::info!(model = %s.server.config.alias, need_mb = config.memory >> 20, budget_mb = budget >> 20, freed_mb = s.server.config.memory >> 20, "modèle déchargé pour faire de la place");
                            s.server.stop().await;
                        }
                        let server = LlamaServer::start(http, config).await?;
                        slots.push(Slot {
                            server,
                            lease: Arc::new(()),
                            last_used: Instant::now(),
                        });
                        return Ok(slots.last_mut().expect("ajouté").endpoint());
                    }
                }
            }
            if started.elapsed() > WAIT_FOR_ROOM {
                bail!(
                    "pas assez de mémoire pour charger {} : les autres modèles sont restés occupés",
                    config.alias
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Alias des modèles chargés, du plus récemment utilisé au plus ancien.
    pub async fn loaded(&self) -> Vec<String> {
        let mut slots = self.slots.lock().await;
        slots.retain_mut(|s| s.server.is_running());
        let mut v: Vec<(Instant, String)> = slots
            .iter()
            .map(|s| (s.last_used, s.server.config.alias.clone()))
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.0));
        v.into_iter().map(|(_, a)| a).collect()
    }

    /// Décharge un modèle (par son alias).
    pub async fn unload_model(&self, alias: &str) {
        let mut slots = self.slots.lock().await;
        for s in extract(&mut slots, |s| s.server.config.alias == alias) {
            s.server.stop().await;
        }
    }

    pub async fn unload(&self) {
        for s in self.slots.lock().await.drain(..) {
            s.server.stop().await;
        }
    }
}

fn extract(slots: &mut Vec<Slot>, pred: impl Fn(&Slot) -> bool) -> Vec<Slot> {
    let (out, keep) = std::mem::take(slots).into_iter().partition(|s| pred(s));
    *slots = keep;
    out
}

/// Libère la place pour `need` en retirant les modèles libres les plus anciens.
/// `None` si la place ne peut pas être faite maintenant (modèles occupés).
fn make_room(slots: &mut Vec<Slot>, need: u64, budget: u64) -> Option<Vec<Slot>> {
    let state: Vec<Loaded> = slots
        .iter()
        .map(|s| Loaded {
            memory: s.server.config.memory,
            idle: s.idle(),
            last_used: s.last_used,
        })
        .collect();
    let mut evict = eviction(&state, need, budget)?;
    evict.sort_unstable_by(|a, b| b.cmp(a));
    Some(evict.into_iter().map(|i| slots.remove(i)).collect())
}

struct Loaded {
    memory: u64,
    idle: bool,
    last_used: Instant,
}

/// Modèles à décharger (indices) pour que `need` tienne dans `budget`. Seul, un
/// modèle est toujours lancé, même au-delà du budget (llama.cpp le répartit
/// alors entre carte graphique et mémoire vive).
fn eviction(loaded: &[Loaded], need: u64, budget: u64) -> Option<Vec<usize>> {
    let mut used: u64 = loaded.iter().map(|l| l.memory).sum();
    let mut order: Vec<usize> = (0..loaded.len()).filter(|&i| loaded[i].idle).collect();
    order.sort_by_key(|&i| loaded[i].last_used);
    let mut evict = Vec::new();
    for i in order {
        if used + need <= budget {
            break;
        }
        used -= loaded[i].memory;
        evict.push(i);
    }
    (used + need <= budget || evict.len() == loaded.len()).then_some(evict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(memory: u64, idle: bool, age_s: u64) -> Loaded {
        Loaded {
            memory,
            idle,
            last_used: Instant::now() - Duration::from_secs(age_s),
        }
    }

    #[test]
    fn keeps_models_that_fit() {
        assert_eq!(eviction(&[l(6, true, 10)], 4, 12), Some(vec![]));
    }

    #[test]
    fn evicts_least_recently_used_idle_model() {
        let loaded = [l(4, true, 5), l(4, true, 50), l(4, false, 100)];
        assert_eq!(eviction(&loaded, 4, 12), Some(vec![1]));
    }

    #[test]
    fn waits_for_busy_models() {
        assert_eq!(eviction(&[l(10, false, 5)], 4, 12), None);
        // Seul modèle à charger : lancé même s'il dépasse.
        assert_eq!(eviction(&[l(10, true, 5)], 20, 12), Some(vec![0]));
        assert_eq!(eviction(&[], 20, 12), Some(vec![]));
    }
}
