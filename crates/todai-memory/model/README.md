# Embedding model

`potion-base-8M` by MinishLab (https://huggingface.co/minishlab/potion-base-8M),
MIT licence, revision `bf8b056651a2c21b8d2565580b8569da283cab23`:

| Source file | sha256 |
|---|---|
| `model.safetensors` | `f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2` |
| `tokenizer.json` | `e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50` |

Converted for todai (`src/embed/potion.rs` reads them, built into the binary):

- `vocab.txt`: the WordPiece vocabulary from `tokenizer.json`, one token per
  line, line number = token id (29,528 tokens).
- `potion-base-8M.i8`: the 29528 × 256 embedding matrix, int8 with one f32
  scale per row (symmetric, scale = max |x| / 127). Layout, little-endian:
  `"TDE1"`, rows u32, dim u32, rows × f32 scales, rows × dim i8. The worst
  row keeps a cosine of 0.9997 to the original.

`src/embed/reference.json` holds token ids from HuggingFace `tokenizers`
0.21.4 and vectors from the official `model2vec-rs` 0.4.0 on the f32 model,
for a fixed list of phrases; the tests compare todai's embedder against them.

To swap the model, see "Changing the embedder" in `docs/memory.md`.
