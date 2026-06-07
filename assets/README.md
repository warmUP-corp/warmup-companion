# Prediction corpus (Leipzig Wortschatz)

Warmup’s n-gram tables are generated from [Leipzig Wortschatz](https://wortschatz-leipzig.de/en/download/eng) corpora (CC BY 4.0 — see their terms).

Normal release builds use the checked-in `src/predict_ngram_prebuilt.bin` file and do **not** rebuild the corpus.

## What to download

From **English → Web** (e.g. [eng-uk_web_2019](https://wortschatz-leipzig.de/en/download/eng#eng-uk_web_2019)) or **News**, pick a size (10K / 100K sentences is enough for the VK prototype).

Extract into this folder:

| File | Used for |
|------|----------|
| `*-words.txt` | Lexicon + unigram frequencies (you already have `eng_news_2025_10K-words.txt`) |
| `*-sentences.txt` | Bigram / trigram statistics (**required** for context ranking) |

The sentence file must share the same prefix as the words file, e.g.:

- `eng_news_2025_10K-words.txt`
- `eng_news_2025_10K-sentences.txt`

`eng-uk_web_2019_100K.tar.gz` works the same way (`eng-uk_web_2019_100K-words.txt` + `…-sentences.txt`).

## Optional

- `assets/ngram_corpus.txt` — small domain sentences (always merged).
- `src/predict_lexicon.txt` — extra VK terms (keyboard, gamepad, …) forced into the lexicon.

## Overrides

```text
LEIPZIG_WORDS=assets/my-words.txt
LEIPZIG_SENTENCES=assets/my-sentences.txt
WARMUP_MAX_LEXICON=12000
WARMUP_MAX_SENTENCES=100000
```

## Rebuilding the prebuilt table

Only do this when the corpus, `assets/ngram_corpus.txt`, `src/predict_lexicon.txt`, or table limits change:

```powershell
$env:WARMUP_REBUILD_NGRAM='1'
$env:WARMUP_WRITE_PREBUILT_NGRAM='1'
cargo check --release
```

After that, normal `cargo build --release` and `.\install\Install-WarmupVk.ps1` runs copy the prebuilt binary table into Cargo’s output directory.

## Git

Large `*-sentences.txt` files are gitignored. Commit `src/predict_ngram_prebuilt.bin` for reproducible builds without downloading or reparsing the corpus.
