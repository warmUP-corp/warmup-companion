//! Build-time n-gram tables for `vk_predict` (ADR 0001).
//!
//! Primary source: [Leipzig Wortschatz](https://wortschatz-leipzig.de/en/download/eng)
//! `*-words.txt` (frequencies) + `*-sentences.txt` (bi/trigram context).
//! See `assets/README.md`.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_MAX_LEXICON: usize = 12_000;
const DEFAULT_MAX_SENTENCES: usize = 100_000;
const DOMAIN_LEXICON: &str = "src/predict_lexicon.txt";
const DOMAIN_CORPUS: &str = "assets/ngram_corpus.txt";
const PREBUILT_NGRAM: &str = "src/predict_ngram_prebuilt.bin";
const ASSETS_DIR: &str = "assets";

fn main() {
    println!("cargo:rerun-if-env-changed=WARMUP_REBUILD_NGRAM");
    println!("cargo:rerun-if-env-changed=WARMUP_WRITE_PREBUILT_NGRAM");
    println!("cargo:rerun-if-changed={PREBUILT_NGRAM}");

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR");
    let out_path = Path::new(&out_dir).join("predict_ngram.bin");

    if !env_flag("WARMUP_REBUILD_NGRAM") {
        if let Ok(prebuilt) = fs::read(PREBUILT_NGRAM) {
            fs::write(&out_path, prebuilt).expect("copy prebuilt predict_ngram.rs");
            return;
        }
    }

    println!("cargo:rerun-if-changed={DOMAIN_LEXICON}");
    println!("cargo:rerun-if-changed={DOMAIN_CORPUS}");
    println!("cargo:rerun-if-changed={ASSETS_DIR}");

    let max_lexicon = env_usize("WARMUP_MAX_LEXICON").unwrap_or(DEFAULT_MAX_LEXICON);
    let max_sentences = env_usize("WARMUP_MAX_SENTENCES").unwrap_or(DEFAULT_MAX_SENTENCES);

    let words_path = discover_words_path();
    let sentences_path = discover_sentences_path(&words_path);

    let leipzig = words_path.as_ref().and_then(|p| load_leipzig_words(p).ok());
    let domain_extra = load_domain_lexicon(Path::new(DOMAIN_LEXICON));
    let lexicon = build_lexicon(leipzig.as_ref(), &domain_extra, max_lexicon);

    let mut tokens = Vec::new();
    if let Some(ref sp) = sentences_path {
        eprintln!(
            "cargo:warning=Leipzig sentences: {} (cap {max_sentences})",
            sp.display()
        );
        tokens.extend(tokenize_sentences_file(sp, &lexicon, max_sentences));
    } else if words_path.is_some() {
        eprintln!(
            "cargo:warning=No *-sentences.txt next to Leipzig words file — \
             bi/trigram quality will be weak. See assets/README.md"
        );
    }
    tokens.extend(tokenize_corpus_text(
        &fs::read_to_string(DOMAIN_CORPUS).unwrap_or_default(),
        &lexicon,
    ));

    let unigram = build_unigram(&lexicon, leipzig.as_ref(), &domain_extra);
    let (bigram_rows, trigram_packed) = count_ngrams(&tokens, lexicon.len());

    let generated = emit_binary(&lexicon, &unigram, &bigram_rows, &trigram_packed);
    fs::write(&out_path, &generated).expect("write predict_ngram.bin");
    if env_flag("WARMUP_WRITE_PREBUILT_NGRAM") {
        fs::write(PREBUILT_NGRAM, generated).expect("write prebuilt predict_ngram.bin");
    }
}

fn env_flag(name: &str) -> bool {
    env::var_os(name).is_some_and(|v| v != "0")
}

fn env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.parse().ok()
}

fn discover_words_path() -> Option<PathBuf> {
    if let Ok(p) = env::var("LEIPZIG_WORDS") {
        return Some(PathBuf::from(p));
    }
    best_words_file_in_assets()
}

fn discover_sentences_path(words: &Option<PathBuf>) -> Option<PathBuf> {
    if let Ok(p) = env::var("LEIPZIG_SENTENCES") {
        return Some(PathBuf::from(p));
    }
    let words = words.as_ref()?;
    let name = words.file_name()?.to_str()?;
    if !name.ends_with("-words.txt") {
        return None;
    }
    let sibling = words.with_file_name(name.replace("-words.txt", "-sentences.txt"));
    sibling.exists().then_some(sibling)
}

fn best_words_file_in_assets() -> Option<PathBuf> {
    let dir = fs::read_dir(ASSETS_DIR).ok()?;
    let mut candidates: Vec<PathBuf> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "txt")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("-words.txt"))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    candidates.pop()
}

/// Leipzig `*-words.txt`: `rank<TAB>word<TAB>count`
fn load_leipzig_words(path: &Path) -> Result<Vec<(String, u32)>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cols = line.split('\t');
        let _rank = cols.next();
        let word = cols.next().unwrap_or("").trim();
        let count: u32 = cols.next().unwrap_or("0").trim().parse().unwrap_or(0);
        if is_lexicon_word(word) {
            out.push((word.to_ascii_lowercase(), count));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

fn load_domain_lexicon(path: &Path) -> HashSet<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|w| is_lexicon_word(w))
        .collect()
}

fn is_lexicon_word(word: &str) -> bool {
    word.len() >= 2 && word.chars().all(|c| c.is_ascii_alphabetic())
}

fn build_lexicon(
    leipzig: Option<&Vec<(String, u32)>>,
    domain_extra: &HashSet<String>,
    max_lexicon: usize,
) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();

    if let Some(freq) = leipzig {
        for (w, _) in freq {
            if set.len() >= max_lexicon.saturating_sub(domain_extra.len()) {
                break;
            }
            set.insert(w.clone());
        }
    }

    for w in domain_extra {
        set.insert(w.clone());
    }

    let mut lexicon: Vec<String> = set.into_iter().collect();
    lexicon.sort();
    lexicon
}

fn build_unigram(
    lexicon: &[String],
    leipzig: Option<&Vec<(String, u32)>>,
    domain_extra: &HashSet<String>,
) -> Vec<u8> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    if let Some(freq) = leipzig {
        for (w, c) in freq {
            counts.insert(w.as_str(), *c);
        }
    }
    let max = counts.values().copied().max().unwrap_or(1);
    let domain_floor = scale(1, max.max(1));

    lexicon
        .iter()
        .map(|w| {
            if let Some(&c) = counts.get(w.as_str()) {
                scale(c, max)
            } else if domain_extra.contains(w) {
                domain_floor.max(32)
            } else {
                0
            }
        })
        .collect()
}

fn word_id(lexicon: &[String], w: &str) -> Option<u16> {
    lexicon
        .binary_search_by(|x| x.as_str().cmp(w))
        .ok()
        .map(|i| i as u16)
}

fn tokenize_sentences_file(path: &Path, lexicon: &[String], cap: usize) -> Vec<u16> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for line in text.lines().take(cap) {
        let sentence = leipzig_sentence_text(line);
        tokens.extend(tokenize_corpus_text(sentence, lexicon));
    }
    tokens
}

/// Leipzig `*-sentences.txt`: `<id>\\t<sentence>`
fn leipzig_sentence_text(line: &str) -> &str {
    line.split_once('\t').map(|x| x.1).unwrap_or(line)
}

fn tokenize_corpus_text(text: &str, lexicon: &[String]) -> Vec<u16> {
    let mut out = Vec::new();
    for line in text.lines() {
        for raw in line.split(|c: char| !c.is_ascii_alphabetic()) {
            let w = raw.trim().to_ascii_lowercase();
            if !is_lexicon_word(&w) {
                continue;
            }
            if let Some(id) = word_id(lexicon, &w) {
                out.push(id);
            }
        }
    }
    out
}

fn count_ngrams(
    tokens: &[u16],
    lex_size: usize,
) -> (Vec<Vec<(u16, u8)>>, Vec<(u16, u16, Vec<(u16, u8)>)>) {
    let mut bi: HashMap<(u16, u16), u32> = HashMap::new();
    let mut tri: HashMap<(u16, u16, u16), u32> = HashMap::new();

    for pair in tokens.windows(2) {
        *bi.entry((pair[0], pair[1])).or_default() += 1;
    }
    for triple in tokens.windows(3) {
        *tri.entry((triple[0], triple[1], triple[2])).or_default() += 1;
    }

    let bigram_rows = group_bigrams(lex_size, bi);
    let trigram_packed = group_trigrams(tri);
    (bigram_rows, trigram_packed)
}

fn scale(count: u32, max: u32) -> u8 {
    if count == 0 {
        return 0;
    }
    let num = (count as f64).ln();
    let den = (max as f64).ln().max(1.0);
    ((num / den) * 255.0).round().clamp(1.0, 255.0) as u8
}

fn group_bigrams(lex_size: usize, counts: HashMap<(u16, u16), u32>) -> Vec<Vec<(u16, u8)>> {
    let mut rows: Vec<Vec<(u16, u8)>> = vec![Vec::new(); lex_size];
    let max_count = counts.values().copied().max().unwrap_or(1);
    for ((prev, next), count) in counts {
        let score = scale(count, max_count);
        let row = &mut rows[prev as usize];
        if let Some(i) = row.iter().position(|e| e.0 == next) {
            row[i].1 = row[i].1.max(score);
        } else {
            row.push((next, score));
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        row.truncate(32);
    }
    rows
}

fn group_trigrams(counts: HashMap<(u16, u16, u16), u32>) -> Vec<(u16, u16, Vec<(u16, u8)>)> {
    let max_count = counts.values().copied().max().unwrap_or(1);
    let mut by_pair: HashMap<(u16, u16), Vec<(u16, u32)>> = HashMap::new();
    for ((w0, w1, w2), count) in counts {
        by_pair.entry((w0, w1)).or_default().push((w2, count));
    }
    let mut out = Vec::new();
    for ((w0, w1), nexts) in by_pair {
        let mut row: Vec<(u16, u8)> = nexts
            .into_iter()
            .map(|(n, c)| (n, scale(c, max_count)))
            .collect();
        row.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        row.truncate(16);
        out.push((w0, w1, row));
    }
    out.sort_by_key(|(a, b, _)| (*a, *b));
    out
}

fn emit_binary(
    lexicon: &[String],
    unigram: &[u8],
    bigram_rows: &[Vec<(u16, u8)>],
    trigram_packed: &[(u16, u16, Vec<(u16, u8)>)],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"WNG1");
    push_u32(&mut out, lexicon.len() as u32);
    for word in lexicon {
        push_u16(&mut out, word.len() as u16);
        out.extend_from_slice(word.as_bytes());
    }
    out.extend_from_slice(unigram);
    for row in bigram_rows {
        push_u16(&mut out, row.len() as u16);
        for (next, score) in row {
            push_u16(&mut out, *next);
            out.push(*score);
        }
    }
    push_u32(&mut out, trigram_packed.len() as u32);
    for (w0, w1, row) in trigram_packed {
        push_u16(&mut out, *w0);
        push_u16(&mut out, *w1);
        push_u16(&mut out, row.len() as u16);
        for (next, score) in row {
            push_u16(&mut out, *next);
            out.push(*score);
        }
    }
    out
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
