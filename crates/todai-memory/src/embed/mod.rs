//! Turning a short phrase into a vector, so a phrase can find the project's
//! existing keys by meaning, not spelling.
//!
//! **This is the one place to change the model.** Everything else talks to
//! the [`Embedder`] trait, and stored vectors are a cache keyed by
//! [`Embedder::id`]: keys stay plain text, so a different embedder just
//! computes its own vectors the first time it is used. No migration, nothing
//! lost, and switching back is free too. To swap:
//!
//! 1. implement [`Embedder`] (wrap another model, a crate, a service...);
//! 2. return it from [`default`];
//! 3. retune its thresholds ([`Embedder::snap`], [`Embedder::ask`],
//!    [`Embedder::recall`]) on real keys: similarity scales differ between
//!    models.

mod potion;
mod wordpiece;
#[cfg(test)]
mod tests;

use std::sync::OnceLock;

pub use potion::Potion;

pub trait Embedder: Send + Sync {
    /// Names the model and everything that changes its vectors (data
    /// format, version of the code). Vectors from different ids are never
    /// compared; change it whenever outputs would change.
    fn id(&self) -> &str;

    /// A unit-length vector, or `None` if nothing in the phrase is known to
    /// the model (empty, or only unknown symbols).
    fn embed(&self, phrase: &str) -> Option<Vec<f32>>;

    /// At or above this similarity, a new key *is* an existing one (a
    /// plural, other casing): adding a card reuses the existing key.
    fn snap(&self) -> f32;

    /// From this up to [`snap`](Self::snap), a new key may be a synonym of
    /// an existing one: adding refuses unless the key is confirmed as new.
    fn ask(&self) -> f32;

    /// Keys at least this similar to a phrase are what recall finds; below
    /// it a phrase has "no key near it".
    fn recall(&self) -> f32;
}

/// The embedder todai uses: potion-base-8M, built into the binary. Loaded
/// on first use (a few ms), shared after.
pub fn default() -> &'static dyn Embedder {
    static E: OnceLock<Potion> = OnceLock::new();
    E.get_or_init(Potion::builtin)
}

/// Cosine similarity of two unit vectors.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
