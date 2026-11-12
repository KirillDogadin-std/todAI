//! Phrase → nearest existing keys → their cards; and snapping a new card's
//! keys onto the vocabulary. Vectors come from an [`Embedder`] and are
//! cached per embedder id in `key_vector`.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::embed::{Embedder, similarity};
use crate::{Card, CardDraft, Class, normalize_key};

/// Keys a phrase may land on; more would flood the result.
const KEYS_PER_PHRASE: usize = 3;
/// Near keys named when a new key is refused.
const NEAR_SHOWN: usize = 3;

/// A card recall found, and which phrases led to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Hit {
    pub card: Card,
    /// One entry per phrase that reached the card: (phrase, key, similarity).
    pub via: Vec<(String, String, f32)>,
}

/// A key `add` replaced by an existing one.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Snap {
    pub from: String,
    pub to: String,
    pub similarity: f32,
}

/// Every key with its vector for `e`, computing the ones not cached yet.
pub(crate) fn vectors(conn: &mut Connection, e: &dyn Embedder) -> Result<Vec<(String, Option<Vec<f32>>)>> {
    let missing: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT k.id, k.phrase FROM key k
             WHERE NOT EXISTS (SELECT 1 FROM key_vector v WHERE v.key = k.id AND v.model = ?1)",
        )?;
        stmt.query_map([e.id()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    if !missing.is_empty() {
        let tx = conn.transaction()?;
        for (id, phrase) in &missing {
            let blob = e.embed(phrase).map(|v| to_blob(&v));
            // Another process may have filled it meanwhile: same result.
            tx.execute("INSERT OR IGNORE INTO key_vector (key, model, vec) VALUES (?1, ?2, ?3)", params![id, e.id(), blob])?;
        }
        tx.commit()?;
    }
    let mut stmt =
        conn.prepare("SELECT k.phrase, v.vec FROM key k JOIN key_vector v ON v.key = k.id AND v.model = ?1")?;
    let rows = stmt.query_map([e.id()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (phrase, blob) = row?;
        out.push((phrase, blob.map(|b| from_blob(&b))));
    }
    Ok(out)
}

/// Existing keys near `phrase`, most similar first, at least `min`. An
/// exact match is 1.0 even if the model knows none of its words.
fn near(phrase: &str, keys: &[(String, Option<Vec<f32>>)], e: &dyn Embedder, min: f32) -> Vec<(String, f32)> {
    let phrase = normalize_key(phrase);
    let v = e.embed(&phrase);
    let mut out: Vec<(String, f32)> = keys
        .iter()
        .filter_map(|(k, kv)| {
            if *k == phrase {
                return Some((k.clone(), 1.0));
            }
            let s = similarity(v.as_ref()?, kv.as_ref()?);
            (s >= min).then(|| (k.clone(), s))
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

pub(crate) fn recall(conn: &mut Connection, phrases: &[String], e: &dyn Embedder) -> Result<Vec<Hit>> {
    let keys = vectors(conn, e)?;
    // card id → (phrase → best (key, similarity))
    let mut reached: HashMap<String, Vec<(String, String, f32)>> = HashMap::new();
    for phrase in phrases.iter().filter(|p| !normalize_key(p).is_empty()) {
        for (key, s) in near(phrase, &keys, e, e.recall()).into_iter().take(KEYS_PER_PHRASE) {
            for card in crate::card::find(conn, std::slice::from_ref(&key))? {
                let via = reached.entry(card.id).or_default();
                match via.iter_mut().find(|(p, _, _)| p == phrase) {
                    Some(v) if v.2 >= s => {}
                    Some(v) => *v = (phrase.clone(), key.clone(), s),
                    None => via.push((phrase.clone(), key.clone(), s)),
                }
            }
        }
    }
    let mut hits = Vec::new();
    for (id, via) in reached {
        if let Some(card) = crate::card::get(conn, &id)? {
            hits.push(Hit { card, via });
        }
    }
    let best = |h: &Hit| h.via.iter().map(|v| v.2).fold(0f32, f32::max);
    let class = |h: &Hit| match h.card.class {
        Class::Axiom => 0,
        Class::Context => 1,
        Class::OneOff => 2,
    };
    hits.sort_by(|a, b| {
        b.via
            .len()
            .cmp(&a.via.len())
            .then(best(b).total_cmp(&best(a)))
            .then(class(a).cmp(&class(b)))
            .then(b.card.created.cmp(&a.card.created))
    });
    Ok(hits)
}

/// Replaces new keys that are an existing key in other words (≥ snap), and
/// refuses ones that may be a synonym (ask band) unless `confirmed`. Every
/// refusal is reported at once.
pub(crate) fn snap(
    conn: &mut Connection,
    mut draft: CardDraft,
    e: &dyn Embedder,
    confirmed: &[String],
) -> Result<(CardDraft, Vec<Snap>)> {
    let confirmed: Vec<String> = confirmed.iter().map(|k| normalize_key(k)).collect();
    let existing = vectors(conn, e)?;
    let mut keys = Vec::new();
    let mut snaps = Vec::new();
    let mut problems = Vec::new();
    for key in std::mem::take(&mut draft.keys) {
        let near = near(&key, &existing, e, e.ask());
        let chosen = match near.first() {
            Some((k, _)) if *k == key => key.clone(),
            Some((k, s)) if *s >= e.snap() => {
                snaps.push(Snap { from: key.clone(), to: k.clone(), similarity: *s });
                k.clone()
            }
            Some(_) if !confirmed.contains(&key) => {
                let shown: Vec<String> = near.iter().take(NEAR_SHOWN).map(|(k, s)| format!("{k:?} ({s:.2})")).collect();
                problems.push(format!(
                    "key {key:?} is close to {}: reuse one, or keep it with --new-key {key:?}",
                    shown.join(", ")
                ));
                continue;
            }
            _ => key.clone(),
        };
        if !keys.contains(&chosen) {
            keys.push(chosen);
        }
    }
    if !problems.is_empty() {
        bail!("keys need a decision:\n- {}", problems.join("\n- "));
    }
    draft.keys = keys;
    Ok((draft, snaps))
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}
