//! The built-in embedder against the official implementations
//! (`reference.json`: HuggingFace `tokenizers` ids, `model2vec-rs` vectors).

use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
struct Reference {
    phrase: String,
    ids: Vec<u32>,
    tokens: Vec<String>,
    vector: Vec<f32>,
}

fn references() -> Vec<Reference> {
    serde_json::from_str(include_str!("reference.json")).unwrap()
}

#[test]
fn tokenizer_matches_huggingface() {
    let p = Potion::builtin();
    for r in references() {
        assert_eq!(p.token_ids(&r.phrase), r.ids, "{:?} should be {:?}", r.phrase, r.tokens);
    }
}

#[test]
fn vectors_match_model2vec_despite_int8() {
    let p = Potion::builtin();
    let mut worst = (1.0f32, String::new());
    for r in references() {
        match p.embed(&r.phrase) {
            None => assert!(r.vector.iter().all(|x| *x == 0.0), "{:?}: reference has a vector", r.phrase),
            Some(v) => {
                assert_eq!(v.len(), r.vector.len());
                let s = similarity(&v, &r.vector);
                if s < worst.0 {
                    worst = (s, r.phrase.clone());
                }
            }
        }
    }
    assert!(worst.0 > 0.999, "worst {worst:?}");
}

#[test]
fn similar_phrases_score_higher_than_unrelated_ones() {
    let e = default();
    let v = |s: &str| e.embed(s).unwrap();
    let sim = |a: &str, b: &str| similarity(&v(a), &v(b));
    assert!((sim("memory usage", "memory usage") - 1.0).abs() < 1e-5);
    assert!(sim("memory usage", "ram usage") > sim("memory usage", "release workflow"));
    assert!(sim("gui toolkit", "ui framework") > sim("gui toolkit", "sqlite wal mode"));
    assert_eq!(e.embed(""), None);
    assert_eq!(e.embed("🚀"), None, "only unknown tokens");
}
