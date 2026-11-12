//! model2vec's static embeddings: a phrase's vector is the mean of its
//! tokens' rows, L2-normalised (unknown tokens skipped), as `model2vec-rs`
//! computes it. The matrix is int8 with a scale per row (see
//! `model/README.md`).

use super::Embedder;
use super::wordpiece::WordPiece;

static MATRIX: &[u8] = include_bytes!("../../model/potion-base-8M.i8");
static VOCAB: &str = include_str!("../../model/vocab.txt");

pub struct Potion {
    tokenizer: WordPiece,
    dim: usize,
    scales: Vec<f32>,
    rows: &'static [u8],
}

impl Potion {
    pub fn builtin() -> Potion {
        Potion::from_parts(MATRIX, VOCAB)
    }

    fn from_parts(matrix: &'static [u8], vocab: &'static str) -> Potion {
        let u32_at = |i: usize| u32::from_le_bytes(matrix[i..i + 4].try_into().unwrap()) as usize;
        assert_eq!(&matrix[..4], b"TDE1", "embedding matrix format");
        let (n, dim) = (u32_at(4), u32_at(8));
        let scales_end = 12 + 4 * n;
        let scales = matrix[12..scales_end].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let rows = &matrix[scales_end..];
        assert_eq!(rows.len(), n * dim, "embedding matrix size");
        let tokenizer = WordPiece::new(vocab);
        Potion { tokenizer, dim, scales, rows }
    }

    pub(crate) fn token_ids(&self, phrase: &str) -> Vec<u32> {
        self.tokenizer.ids(phrase)
    }
}

impl Embedder for Potion {
    fn id(&self) -> &str {
        "potion-base-8M/int8/1"
    }

    fn embed(&self, phrase: &str) -> Option<Vec<f32>> {
        let mut sum = vec![0f32; self.dim];
        let mut n = 0;
        for id in self.token_ids(phrase) {
            if id == self.tokenizer.unk() {
                continue;
            }
            let id = id as usize;
            let scale = self.scales[id];
            for (s, &q) in sum.iter_mut().zip(&self.rows[id * self.dim..(id + 1) * self.dim]) {
                *s += q as i8 as f32 * scale;
            }
            n += 1;
        }
        if n == 0 {
            return None;
        }
        // The mean's direction is the sum's: normalising is enough.
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return None;
        }
        sum.iter_mut().for_each(|x| *x /= norm);
        Some(sum)
    }

    // Measured on 33 key pairs (plans step 03 log; board memory/d22):
    // plurals and case score 0.89-1.0, synonyms 0.31-0.75, related but
    // different keys 0.52-0.74, unrelated 0.02-0.24.
    fn snap(&self) -> f32 {
        0.88
    }

    fn ask(&self) -> f32 {
        0.65
    }

    fn recall(&self) -> f32 {
        0.5
    }
}
