//! Mémoire de Claudio : index des documents autorisés (recherche hybride
//! vecteurs + plein texte), souvenirs à long terme et compétences apprises.

pub mod index;
pub mod memories;
pub mod skills;
mod vectors;

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

pub use vectors::{ensure_vectors, fts_query};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Fournit des embeddings normalisés (modèle local).
pub trait Embedder: Send + Sync {
    /// Identifiant stable du modèle (un changement impose de tout recalculer).
    fn model(&self) -> String;
    fn dims(&self) -> usize;
    /// `query` : le texte est une question (certains modèles veulent un préfixe).
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
        query: bool,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;
}

/// Fusion par rang réciproque : combine des classements sans comparer leurs scores.
pub(crate) fn rrf(lists: &[Vec<i64>]) -> Vec<(i64, f64)> {
    let mut scores: std::collections::HashMap<i64, f64> = Default::default();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
        }
    }
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Embedder de test : sac de mots haché sur 64 dimensions.
    pub struct Hashing;

    pub fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0f32; 64];
        for w in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
        {
            let h = w
                .bytes()
                .fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
            v[(h % 64) as usize] += 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    impl Embedder for Hashing {
        fn model(&self) -> String {
            "hachage-test".into()
        }
        fn dims(&self) -> usize {
            64
        }
        fn embed<'a>(
            &'a self,
            texts: &'a [String],
            _query: bool,
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            Box::pin(async move { Ok(texts.iter().map(|t| vector(t)).collect()) })
        }
    }

    #[test]
    fn rrf_prefers_items_ranked_by_both() {
        let r = rrf(&[vec![1, 2, 3], vec![3, 1]]);
        assert_eq!(r[0].0, 1);
        assert_eq!(r[1].0, 3);
    }
}
