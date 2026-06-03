//! Embedded n-gram tables, prebuilt by `build.rs`.

use std::sync::OnceLock;

static DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/predict_ngram.bin"));
static TABLES: OnceLock<Tables> = OnceLock::new();

struct Tables {
    lexicon: Vec<&'static str>,
    unigram: Vec<u8>,
    bigram_next: Vec<Vec<u16>>,
    bigram_score: Vec<Vec<u8>>,
    trigram_pair: Vec<(u16, u16)>,
    trigram_next: Vec<Vec<u16>>,
    trigram_score: Vec<Vec<u8>>,
}

pub fn lexicon() -> &'static [&'static str] {
    &tables().lexicon
}

pub fn word_id(word: &str) -> Option<u16> {
    tables().lexicon.binary_search(&word).ok().map(|i| i as u16)
}

pub fn unigram_score(id: u16) -> u32 {
    tables().unigram.get(id as usize).copied().unwrap_or(0) as u32
}

pub fn bigram_score(prev: u16, next: u16) -> u32 {
    let t = tables();
    let (row, scores) = match (
        t.bigram_next.get(prev as usize),
        t.bigram_score.get(prev as usize),
    ) {
        (Some(r), Some(s)) => (r, s),
        _ => return 0,
    };
    row.iter()
        .position(|&n| n == next)
        .map(|i| scores[i] as u32)
        .unwrap_or(0)
}

pub fn trigram_score(w0: u16, w1: u16, next: u16) -> u32 {
    let t = tables();
    let i = match t.trigram_pair.binary_search_by_key(&(w0, w1), |p| *p) {
        Ok(i) => i,
        Err(_) => return 0,
    };
    let (row, scores) = match (t.trigram_next.get(i), t.trigram_score.get(i)) {
        (Some(r), Some(s)) => (r, s),
        _ => return 0,
    };
    row.iter()
        .position(|&n| n == next)
        .map(|j| scores[j] as u32)
        .unwrap_or(0)
}

pub fn rank_score(prev: Option<u16>, prev2: Option<u16>, next: u16, personal: bool) -> u32 {
    let mut score = unigram_score(next);
    if let Some(p) = prev {
        score += bigram_score(p, next) * 4;
    }
    if let (Some(p0), Some(p1)) = (prev2, prev) {
        score += trigram_score(p0, p1, next) * 6;
    }
    if personal {
        score += 10_000;
    }
    score
}

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| parse_tables(DATA).expect("invalid embedded n-gram table"))
}

fn parse_tables(data: &'static [u8]) -> Result<Tables, String> {
    let mut r = Reader { data, pos: 0 };
    if r.take(4)? != b"WNG1" {
        return Err("bad magic".into());
    }
    let lex_len = r.u32()? as usize;
    let mut lexicon = Vec::with_capacity(lex_len);
    for _ in 0..lex_len {
        let len = r.u16()? as usize;
        let bytes = r.take(len)?;
        let word = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        lexicon.push(word);
    }
    let unigram = r.take(lex_len)?.to_vec();

    let mut bigram_next = Vec::with_capacity(lex_len);
    let mut bigram_score = Vec::with_capacity(lex_len);
    for _ in 0..lex_len {
        let row_len = r.u16()? as usize;
        let mut next = Vec::with_capacity(row_len);
        let mut score = Vec::with_capacity(row_len);
        for _ in 0..row_len {
            next.push(r.u16()?);
            score.push(r.u8()?);
        }
        bigram_next.push(next);
        bigram_score.push(score);
    }

    let tri_len = r.u32()? as usize;
    let mut trigram_pair = Vec::with_capacity(tri_len);
    let mut trigram_next = Vec::with_capacity(tri_len);
    let mut trigram_score = Vec::with_capacity(tri_len);
    for _ in 0..tri_len {
        trigram_pair.push((r.u16()?, r.u16()?));
        let row_len = r.u16()? as usize;
        let mut next = Vec::with_capacity(row_len);
        let mut score = Vec::with_capacity(row_len);
        for _ in 0..row_len {
            next.push(r.u16()?);
            score.push(r.u8()?);
        }
        trigram_next.push(next);
        trigram_score.push(score);
    }

    Ok(Tables {
        lexicon,
        unigram,
        bigram_next,
        bigram_score,
        trigram_pair,
        trigram_next,
        trigram_score,
    })
}

struct Reader {
    data: &'static [u8],
    pos: usize,
}

impl Reader {
    fn take(&mut self, len: usize) -> Result<&'static [u8], String> {
        let end = self.pos.checked_add(len).ok_or("table offset overflow")?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or("truncated n-gram table")?;
        self.pos = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}
