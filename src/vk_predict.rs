//! Local prefix prediction: VK-only context, userland only.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::predict_dict;
use crate::predict_ngram;

const MIN_PREFIX_LEN: usize = 1;
const MAX_CANDIDATES: usize = 7;
const SENTENCE_STARTERS: &[&str] = &["I", "The", "I'm", "Thanks", "Hi", "It", "We"];

fn new_state() -> PredictState {
    PredictState {
        enabled: false,
        words: Vec::new(),
        partial: String::new(),
        ranked: Vec::new(),
        highlight: 0,
        candidate_engaged: false,
        personal: HashSet::new(),
        sentence_start: true,
        context_known: true,
        slots: MAX_CANDIDATES,
        mono: false,
    }
}

static STATE: std::sync::LazyLock<Mutex<PredictState>> =
    std::sync::LazyLock::new(|| Mutex::new(new_state()));

struct PredictState {
    enabled: bool,
    words: Vec<String>,
    partial: String,
    ranked: Vec<String>,
    highlight: usize,
    /// True after LB/RB cycle while the strip is showing (A may commit).
    candidate_engaged: bool,
    personal: HashSet<String>,
    sentence_start: bool,
    context_known: bool,
    slots: usize,
    mono: bool,
}

/// Single source of truth for the candidate strip: the visible chips, which slot
/// is highlighted, and whether the user engaged the strip with LB/RB (so A may
/// commit). `strip()` returns this; `None` means no strip should show.
pub struct StripState {
    pub visible: Vec<String>,
    pub highlight_slot: usize,
    pub engaged: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Choice {
    label: String,
    insert: String,
    literal: bool,
}

impl Choice {
    fn word(w: String) -> Self {
        Choice {
            label: w.clone(),
            insert: w,
            literal: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.label.is_empty()
    }
}

fn lexicon() -> &'static [&'static str] {
    predict_ngram::lexicon()
}

fn words_with_prefix(prefix: &str) -> impl Iterator<Item = &str> {
    let lex = lexicon();
    let start = lex.partition_point(|w| *w < prefix);
    lex[start..]
        .iter()
        .copied()
        .take_while(move |w| w.starts_with(prefix))
}

pub fn predictions_enabled() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(not(test))]
    {
        crate::win::surface::input()
            .map(|s| s.is_userland())
            .unwrap_or(false)
    }
}

pub fn reset() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    s.words.clear();
    s.partial.clear();
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
    s.sentence_start = true;
    s.context_known = true;
    s.enabled = predictions_enabled();
    predict_dict::load_personal(&mut s.personal);
    refresh_ranked(&mut s);
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn wants_capital(s: &PredictState) -> bool {
    s.sentence_start
        || s
            .partial
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
}

fn choices(s: &PredictState) -> Vec<Choice> {
    let cap = wants_capital(s);
    let cased = |w: &String| Choice::word(if cap { capitalize(w) } else { w.clone() });
    if !s.mono {
        return s.ranked.iter().take(MAX_CANDIDATES).map(cased).collect();
    }
    let slots = s.slots.max(1);
    if s.partial.is_empty() {
        return s.ranked.iter().take(slots).map(cased).collect();
    }
    let literal = Choice {
        label: format!("\u{201C}{}\u{201D}", s.partial),
        insert: s.partial.clone(),
        literal: true,
    };
    std::iter::once(literal)
        .chain(
            s.ranked
                .iter()
                .filter(|w| !w.eq_ignore_ascii_case(&s.partial))
                .take(slots - 1)
                .map(cased),
        )
        .collect()
}

fn default_highlight(s: &PredictState) -> usize {
    let list = choices(s);
    let preferred = usize::from(list.first().is_some_and(|c| c.literal));
    if list.get(preferred).is_some_and(|c| !c.is_empty()) {
        return preferred;
    }
    list.iter().position(|c| !c.is_empty()).unwrap_or(0)
}

fn refresh_ranked(s: &mut PredictState) {
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
    if !s.enabled {
        return;
    }
    if s.partial.is_empty() {
        if s.context_known {
            rank_next_words(s);
        }
    } else if s.partial.len() >= MIN_PREFIX_LEN {
        rank_prefix(s);
    }
    s.highlight = default_highlight(s);
}

fn context_ids(s: &PredictState) -> (Option<u16>, Option<u16>) {
    let n = s.words.len();
    let prev = s.words.last().and_then(|w| predict_ngram::word_id(w));
    let prev2 = if n >= 2 {
        predict_ngram::word_id(&s.words[n - 2])
    } else {
        None
    };
    (prev, prev2)
}

fn rank_prefix(s: &mut PredictState) {
    let prefix = s.partial.to_ascii_lowercase();
    let prefix = prefix.as_str();
    let (prev, prev2) = context_ids(s);

    let mut scored: Vec<(u32, String)> = Vec::new();
    for word in words_with_prefix(prefix) {
        let Some(id) = predict_ngram::word_id(word) else {
            continue;
        };
        let personal = s.personal.contains(word);
        let score = predict_ngram::rank_score(prev, prev2, id, personal);
        scored.push((score, word.to_string()));
    }
    for word in &s.personal {
        if word.starts_with(prefix) && !scored.iter().any(|(_, w)| w == word) {
            let score = predict_ngram::rank_score(
                prev,
                prev2,
                predict_ngram::word_id(word).unwrap_or(0),
                true,
            );
            scored.push((score, word.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, w) in scored.into_iter().take(MAX_CANDIDATES) {
        s.ranked.push(w);
    }
}

fn rank_next_words(s: &mut PredictState) {
    if s.sentence_start {
        s.ranked
            .extend(SENTENCE_STARTERS.iter().map(|w| w.to_string()));
        return;
    }
    let (prev, prev2) = context_ids(s);
    let mut ids: Vec<u16> = Vec::new();
    if let (Some(p0), Some(p1)) = (prev2, prev) {
        ids.extend_from_slice(predict_ngram::trigram_row(p0, p1));
    }
    if let Some(p) = prev {
        ids.extend_from_slice(predict_ngram::bigram_row(p));
    }
    ids.sort_unstable();
    ids.dedup();
    let mut scored: Vec<(u32, u16)> = ids
        .into_iter()
        .map(|id| (predict_ngram::rank_score(prev, prev2, id, false), id))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let lex = lexicon();
    let fill = predict_ngram::top_unigrams().iter().map(|id| (0, *id));
    for (_, id) in scored.into_iter().chain(fill) {
        if s.ranked.len() >= MAX_CANDIDATES {
            break;
        }
        if let Some(w) = lex.get(id as usize) {
            if !s.ranked.iter().any(|r| r == w) {
                s.ranked.push(w.to_string());
            }
        }
    }
}

/// The current candidate strip, or `None` when no strip should show. The one
/// query for both rendering and the LB/RB context-swap decision.
pub fn strip(slots: usize, mono: bool) -> Option<StripState> {
    let mut s = STATE.lock().ok()?;
    let slots = slots.max(1);
    if s.slots != slots || s.mono != mono {
        s.slots = slots;
        s.mono = mono;
        s.highlight = default_highlight(&s);
    }
    if !strip_active_inner(&s) {
        return None;
    }
    let list = choices(&s);
    let start = viewport_start(s.highlight, list.len(), slots);
    let visible = (0..slots)
        .map(|i| {
            list.get(start + i)
                .map(|c| c.label.clone())
                .unwrap_or_default()
        })
        .collect();
    let highlight_slot = s.highlight.saturating_sub(start);
    Some(StripState {
        visible,
        highlight_slot,
        engaged: s.candidate_engaged,
    })
}

fn strip_active_inner(s: &PredictState) -> bool {
    s.enabled && !s.ranked.is_empty()
}

fn viewport_start(highlight: usize, total: usize, slots: usize) -> usize {
    if total <= slots {
        return 0;
    }
    highlight.saturating_sub(slots / 2).min(total - slots)
}

fn step_highlight(s: &mut PredictState, forward: bool) -> bool {
    if !strip_active_inner(s) {
        return false;
    }
    let list = choices(s);
    let n = list.len();
    if n == 0 {
        return false;
    }
    let mut i = s.highlight.min(n - 1);
    for _ in 0..n {
        i = if forward { (i + 1) % n } else { (i + n - 1) % n };
        if !list[i].is_empty() {
            break;
        }
    }
    s.highlight = i;
    s.candidate_engaged = true;
    true
}

pub fn cycle_next() -> bool {
    let Ok(mut s) = STATE.lock() else {
        return false;
    };
    step_highlight(&mut s, true)
}

pub fn cycle_prev() -> bool {
    let Ok(mut s) = STATE.lock() else {
        return false;
    };
    step_highlight(&mut s, false)
}

pub fn engage() -> bool {
    let Ok(mut s) = STATE.lock() else {
        return false;
    };
    if !strip_active_inner(&s) {
        return false;
    }
    s.candidate_engaged = true;
    true
}

pub fn disengage() {
    if let Ok(mut s) = STATE.lock() {
        s.candidate_engaged = false;
    }
}

pub fn strip_engaged() -> bool {
    let Ok(s) = STATE.lock() else {
        return false;
    };
    s.candidate_engaged && strip_active_inner(&s)
}

fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\n' | '\r')
}

pub fn on_char(c: char) {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    s.enabled = predictions_enabled();
    if !s.enabled {
        return;
    }
    if c.is_ascii_alphabetic() {
        s.partial.push(c);
    } else if c.is_ascii_digit() || c == '_' {
        finish_word(&mut s);
        s.partial.push(c);
    } else if ends_sentence(c) {
        finish_word(&mut s);
        start_sentence(&mut s);
    } else {
        finish_word(&mut s);
        s.context_known = true;
    }
    refresh_ranked(&mut s);
}

pub fn on_backspace() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    if s.partial.pop().is_none() {
        s.words.clear();
        s.sentence_start = false;
        s.context_known = false;
    }
    refresh_ranked(&mut s);
}

pub fn on_space() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    finish_word(&mut s);
    s.context_known = true;
    refresh_ranked(&mut s);
}

pub fn on_boundary() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    finish_word(&mut s);
    start_sentence(&mut s);
    refresh_ranked(&mut s);
}

pub fn on_caret_move() {
    let Ok(mut s) = STATE.lock() else {
        return;
    };
    s.partial.clear();
    s.words.clear();
    s.sentence_start = false;
    s.context_known = false;
    clear_strip(&mut s);
}

fn start_sentence(s: &mut PredictState) {
    s.words.clear();
    s.sentence_start = true;
    s.context_known = true;
}

fn finish_word(s: &mut PredictState) {
    if !s.partial.is_empty() {
        s.sentence_start = false;
    }
    if s.partial.len() >= 2 {
        let w = std::mem::take(&mut s.partial).to_ascii_lowercase();
        record_completed(s, &w);
    }
    clear_strip(s);
}

/// Record a completed word: password-gated personal-dict learn (CONTEXT.md
/// "Secure field") + push to the VK-only context buffer (cap 8). Shared by
/// finish_word and Candidate commit so "word completed" lives in one place.
fn record_completed(s: &mut PredictState, word: &str) {
    maybe_learn(s, word);
    s.words.push(word.to_string());
    if s.words.len() > 8 {
        s.words.remove(0);
    }
}

/// Clear the partial + candidate strip after a word boundary or commit.
fn clear_strip(s: &mut PredictState) {
    s.partial.clear();
    s.ranked.clear();
    s.highlight = 0;
    s.candidate_engaged = false;
}

fn maybe_learn(s: &mut PredictState, word: &str) {
    let w = word.to_ascii_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return;
    }
    // Password fields must not train the personal dict (CONTEXT.md "Secure field").
    // The focus probe lives in logon_focus; treat unknown as unsafe.
    if !safe_to_learn_from_focus() {
        return;
    }
    if s.personal.insert(w) {
        predict_dict::flush_personal(&s.personal);
    }
}

fn safe_to_learn_from_focus() -> bool {
    matches!(
        crate::win::logon_focus::focused_is_password_field(),
        Some(false)
    )
}

/// Commit the highlighted candidate through `sink` (CONTEXT.md "Candidate
/// commit"), but only when the user engaged the strip with LB/RB first. Deletes
/// the partial prefix and injects the chosen word as one Text-commit replace.
/// Returns the outcome, or None when there is nothing to commit. The engage gate
/// and the word pick share one lock, so a stale strip cannot commit. Personal-
/// dict learn + VK-buffer push happen only on a landed commit.
pub fn commit_if_engaged(
    sink: &mut dyn crate::vk_commit::TextSink,
) -> Option<crate::vk_commit::Committed> {
    commit_choice(sink, |s| s.candidate_engaged.then_some(s.highlight))
}

pub fn commit_slot(
    slot: usize,
    sink: &mut dyn crate::vk_commit::TextSink,
) -> Option<crate::vk_commit::Committed> {
    commit_choice(sink, |s| {
        let n = choices(s).len();
        Some(viewport_start(s.highlight, n, s.slots.max(1)) + slot)
    })
}

fn commit_choice(
    sink: &mut dyn crate::vk_commit::TextSink,
    pick: impl FnOnce(&PredictState) -> Option<usize>,
) -> Option<crate::vk_commit::Committed> {
    let (choice, del) = {
        let s = STATE.lock().ok()?;
        if !strip_active_inner(&s) {
            return None;
        }
        let idx = pick(&s)?;
        let choice = choices(&s).into_iter().nth(idx)?;
        if choice.is_empty() {
            return None;
        }
        let del = if choice.literal {
            0
        } else {
            s.partial.chars().count()
        };
        (choice, del)
    };
    let res = if choice.literal {
        crate::vk_commit::Committed {
            word: choice.insert.clone(),
            deleted: 0,
            injected: sink.replace(0, " ").is_ok(),
        }
    } else {
        let res = crate::vk_commit::commit(&choice.insert, del, sink);
        if res.injected {
            // After accepting a chip, append a space so the next word starts cleanly
            // (as if the user typed the word then pressed space). Only the field text
            // gets the space — the learned word / VK context buffer below stays clean.
            let _ = sink.replace(0, " ");
        }
        res
    };
    if let Ok(mut s) = STATE.lock() {
        clear_strip(&mut s);
        if res.injected {
            let word = choice.insert.to_ascii_lowercase();
            if !choice.literal || word.len() >= 2 {
                record_completed(&mut s, &word);
            }
            s.sentence_start = false;
            s.context_known = true;
            refresh_ranked(&mut s);
        }
    }
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn type_word(s: &str) {
        for c in s.chars() {
            on_char(c);
        }
        on_space();
    }

    fn type_chars(s: &str) {
        for c in s.chars() {
            on_char(c);
        }
    }

    fn fresh(slots: usize) {
        reset();
        let _ = strip(slots, false);
    }

    fn fresh_mono() {
        reset();
        let _ = strip(7, true);
    }

    fn highlighted_insert() -> String {
        let s = STATE.lock().unwrap();
        choices(&s)[s.highlight].insert.clone()
    }

    #[test]
    fn prefix_finds_keyboard() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(3);
        type_chars("keyb");
        assert!(strip(3, false).is_some());
        let ranked = STATE.lock().unwrap().ranked.clone();
        assert!(ranked.iter().any(|w| w == "keyboard"));
    }

    #[test]
    fn bigram_prefers_in_after_the() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        type_word("the");
        on_char('i');
        on_char('n');
        let ranked = STATE.lock().unwrap().ranked.clone();
        assert!(!ranked.is_empty(), "ranked: {ranked:?}");
        assert_eq!(ranked[0], "in", "ranked: {ranked:?}");
    }

    #[test]
    fn caret_move_drops_partial_word() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(3);
        type_chars("keyb");
        on_caret_move();
        assert!(strip(3, false).is_none());
        assert!(STATE.lock().unwrap().words.is_empty());
    }

    #[test]
    fn engage_and_disengage_strip() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        on_caret_move();
        assert!(!engage());
        type_chars("keyb");
        assert!(engage());
        assert!(strip_engaged());
        disengage();
        assert!(!strip_engaged());
    }

    #[test]
    fn viewport_at_end() {
        assert_eq!(viewport_start(4, 5, 3), 2);
        assert_eq!(viewport_start(0, 5, 3), 0);
        assert_eq!(viewport_start(1, 5, 3), 0);
        assert_eq!(viewport_start(2, 5, 3), 1);
        assert_eq!(viewport_start(3, 5, 3), 2);
        assert_eq!(viewport_start(6, 7, 7), 0);
        assert_eq!(viewport_start(6, 7, 3), 4);
    }

    #[test]
    fn a_does_not_commit_until_shoulder_cycle() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(3);
        type_chars("keyb");
        assert!(strip(3, false).is_some());
        assert!(!strip(3, false).unwrap().engaged);
        let mut sink = crate::vk_commit::BufSink::new("keyb");
        assert!(commit_if_engaged(&mut sink).is_none());
        assert_eq!(sink.buf, "keyb");
        assert!(cycle_next());
        assert!(strip(3, false).unwrap().engaged);
    }

    #[test]
    fn commit_replaces_prefix_with_word() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        type_word("the");
        type_chars("keyb");
        cycle_next();
        let highlighted = highlighted_insert();
        let mut sink = crate::vk_commit::BufSink::new("keyb");
        let res = commit_if_engaged(&mut sink).expect("engaged commit");
        assert!(res.injected);
        assert_eq!(res.deleted, 4);
        assert_eq!(sink.buf, format!("{highlighted} "));
        assert_eq!(STATE.lock().unwrap().words.last().unwrap(), &highlighted);
    }

    #[test]
    fn failed_inject_does_not_record() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        on_caret_move();
        type_chars("keyb");
        cycle_next();
        let mut sink = crate::vk_commit::BufSink::failing("keyb");
        let res = commit_if_engaged(&mut sink).expect("attempted commit");
        assert!(!res.injected);
        assert_eq!(sink.buf, "keyb");
        assert!(STATE.lock().unwrap().words.is_empty());
    }

    #[test]
    fn opening_shows_capitalized_sentence_starters() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        let strip = strip(7, false).expect("strip shows before typing");
        assert!(strip.visible.iter().any(|w| w == "I"), "{:?}", strip.visible);
        assert!(strip
            .visible
            .iter()
            .filter(|w| !w.is_empty())
            .all(|w| w.chars().next().unwrap().is_uppercase()));
    }

    #[test]
    fn partial_at_sentence_start_is_capitalized() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        type_chars("keyb");
        let visible = strip(7, false).unwrap().visible;
        assert!(visible.iter().any(|w| w == "Keyboard"), "{visible:?}");
        type_word("");
        on_char('.');
        on_space();
        let visible = strip(7, false).unwrap().visible;
        assert!(visible.iter().any(|w| w == "The"), "{visible:?}");
    }

    #[test]
    fn next_word_follows_thank() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        type_word("thank");
        let visible = strip(7, false).expect("next-word strip").visible;
        assert!(visible.iter().any(|w| w == "you"), "{visible:?}");
    }

    #[test]
    fn empty_partial_commit_inserts_word_and_space() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh(7);
        type_word("thank");
        {
            let mut s = STATE.lock().unwrap();
            let idx = choices(&s).iter().position(|c| c.insert == "you").unwrap();
            s.highlight = idx;
            s.candidate_engaged = true;
        }
        let mut sink = crate::vk_commit::BufSink::new("thank ");
        let res = commit_if_engaged(&mut sink).expect("engaged commit");
        assert!(res.injected);
        assert_eq!(res.deleted, 0);
        assert_eq!(sink.buf, "thank you ");
        assert!(strip(7, false).is_some(), "next-word strip after commit");
        let words = STATE.lock().unwrap().words.clone();
        assert_eq!(words, vec!["thank".to_string(), "you".to_string()]);
    }

    #[test]
    fn mono_puts_best_prediction_first_and_highlights_it() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh_mono();
        type_word("thank");
        let ranked = STATE.lock().unwrap().ranked.clone();
        let strip = strip(7, true).unwrap();
        assert_eq!(strip.visible.len(), 7);
        let n = ranked.len().min(7);
        assert_eq!(strip.visible[..n], ranked[..n]);
        assert_eq!(strip.highlight_slot, 0);
    }

    #[test]
    fn mono_typing_shows_literal_left_and_commits_it() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh_mono();
        type_word("the");
        type_chars("keyb");
        let ranked = STATE.lock().unwrap().ranked.clone();
        let strip = strip(7, true).unwrap();
        assert_eq!(strip.visible.len(), 7);
        assert_eq!(strip.visible[0], "\u{201C}keyb\u{201D}");
        assert_eq!(strip.visible[1], ranked[0]);
        assert_eq!(strip.visible[1], "keyboard");
        assert_eq!(strip.highlight_slot, 1);
        assert!(strip.visible[2..]
            .iter()
            .all(|w| w != "\u{201C}keyb\u{201D}"));
        let mut sink = crate::vk_commit::BufSink::new("the keyb");
        let res = commit_slot(0, &mut sink).expect("literal commit");
        assert!(res.injected);
        assert_eq!(sink.buf, "the keyb ");
        assert_eq!(STATE.lock().unwrap().words.last().unwrap(), "keyb");
    }

    #[test]
    fn mono_commit_slot_picks_a_later_chip() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh_mono();
        type_word("the");
        type_chars("th");
        let visible = strip(7, true).unwrap().visible;
        assert!(visible.iter().all(|w| !w.is_empty()), "{visible:?}");
        let word = visible[6].clone();
        let mut sink = crate::vk_commit::BufSink::new("the th");
        let res = commit_slot(6, &mut sink).expect("chip commit");
        assert!(res.injected);
        assert_eq!(sink.buf, format!("the {word} "));
    }

    #[test]
    fn mono_cycle_wraps_across_all_slots() {
        let _g = TEST_LOCK.lock().unwrap();
        fresh_mono();
        type_word("thank");
        let n = strip(7, true)
            .unwrap()
            .visible
            .iter()
            .filter(|w| !w.is_empty())
            .count();
        assert_eq!(n, 7);
        for expected in 1..n {
            assert!(cycle_next());
            assert_eq!(strip(7, true).unwrap().highlight_slot, expected);
        }
        assert!(cycle_next());
        assert_eq!(strip(7, true).unwrap().highlight_slot, 0);
        assert!(cycle_prev());
        assert_eq!(strip(7, true).unwrap().highlight_slot, n - 1);
    }
}
