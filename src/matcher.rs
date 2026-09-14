use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use regex::{Regex, RegexBuilder};
use rustmigemo::migemo::compact_dictionary::CompactDictionary;
use rustmigemo::migemo::query::query;
use rustmigemo::migemo::regex_generator::RegexOperator;
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};

use crate::window_list::TaskWindow;

// BSD-derived (Mozc + UniDic), see assets/LICENSE*.
static DICT_BYTES: &[u8] = include_bytes!("../assets/migemo-compact-dict.bin");

/// How many recently typed queries keep their compiled form when
/// `regexcache=` is not set.
pub const DEFAULT_CACHE_SIZE: usize = 100;

fn dictionary() -> &'static CompactDictionary {
    static DICT: OnceLock<CompactDictionary> = OnceLock::new();
    DICT.get_or_init(|| CompactDictionary::new(&DICT_BYTES.to_vec()))
}

/// A search query compiled once, then tested against every row.
#[derive(Clone)]
pub enum CompiledQuery {
    /// Blank query, or migemo generated an empty pattern: everything matches.
    All,
    Regex(Arc<Regex>),
    /// The generated pattern failed to compile; holds the lowercased query
    /// for a plain substring check instead.
    Substring(String),
}

impl CompiledQuery {
    fn matches_text(&self, haystack: &str) -> bool {
        match self {
            CompiledQuery::All => true,
            CompiledQuery::Regex(re) => re.is_match(haystack),
            CompiledQuery::Substring(needle) => haystack.to_lowercase().contains(needle),
        }
    }

    /// A window matches if the query matches its title or its resolved
    /// friendly app name.
    pub fn matches(&self, window: &TaskWindow) -> bool {
        self.matches_text(&window.title) || self.matches_text(&window.friendly_name)
    }
}

/// A search box query, split on whitespace into independently compiled
/// terms. A window matches only if every term matches (AND), which lets a
/// query like "afx 2" find "2) afx" even though no single migemo pattern
/// could express "both of these, in any order, anywhere". Each term is
/// cached under its own text (see `compile_term`), so "afx 2" and "afx 3"
/// share the cached "afx" compile.
#[derive(Clone)]
pub struct Query(Vec<CompiledQuery>);

impl Query {
    pub fn matches(&self, window: &TaskWindow) -> bool {
        self.0.iter().all(|term| term.matches(window))
    }
}

/// Compiles `query_text` into a migemo regex (romaji -> kana/kanji reading
/// expansion). Case-insensitive: migemo's own query() keeps the literal
/// query text in whatever case was typed (e.g. "c"), and Japanese has no
/// case anyway, so a case-sensitive compile would fail to match e.g.
/// "Claude" against a lowercase "c" query. Falls back to a plain
/// case-insensitive substring check if the generated pattern fails to
/// compile.
///
/// Expensive for short queries: a single letter expands to every reading
/// that starts with it, a pattern of 15,000-25,000 characters that took
/// query() between ~30 ms and ~230 ms to generate depending on the build
/// (see ROADMAP B1). Longer queries take well under 2 ms. That is what the
/// cache and the a-z prewarm below exist for.
fn build(query_text: &str) -> CompiledQuery {
    let pattern = query(query_text.to_string(), dictionary(), &RegexOperator::Default);
    if pattern.is_empty() {
        return CompiledQuery::All;
    }
    match RegexBuilder::new(&pattern).case_insensitive(true).build() {
        Ok(re) => CompiledQuery::Regex(Arc::new(re)),
        Err(e) => {
            crate::log::log(&format!("matcher: generated pattern failed to compile: {}", e));
            CompiledQuery::Substring(query_text.to_lowercase())
        }
    }
}

/// Splits `query_text` on whitespace (including full-width `　`) and
/// compiles each word as a separate AND'd term; see `Query`. A blank query
/// yields no terms, which `Query::matches` treats as matching everything.
pub fn compile(query_text: &str) -> Query {
    Query(query_text.split_whitespace().map(compile_term).collect())
}

/// Compiles a single term, reusing an earlier compile of the same text when
/// the cache still holds one.
fn compile_term(query_text: &str) -> CompiledQuery {
    if let Some(hit) = cache().get(query_text) {
        return hit;
    }
    // Built without holding the lock, so the prewarm thread and the popup
    // never wait on each other's compiles.
    let compiled = build(query_text);
    cache().insert_recent(query_text.to_string(), compiled.clone());
    compiled
}

/// Sets how many recently typed queries stay compiled; 0 turns the cache
/// off. Must be called before the popup can be opened.
pub fn set_cache_size(capacity: usize) {
    cache().capacity = capacity;
}

/// Compiles the 26 single-letter queries on a low-priority background
/// thread. Every search starts with one of them and they are by far the
/// slowest to build, so this removes the stall on the first keystroke. It
/// also builds the dictionary, which would otherwise happen then too. The
/// results are kept whether or not the recent-query cache is enabled.
pub fn prewarm_in_background() {
    std::thread::spawn(|| {
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        let started = Instant::now();
        for letter in 'a'..='z' {
            let key = letter.to_string();
            // Typed before the prewarm got to it: promote it rather than rebuild.
            let already_built = cache().recent.remove(&key).map(|(compiled, _)| compiled);
            let compiled = already_built.unwrap_or_else(|| build(&key));
            cache().insert_prewarmed(key, compiled);
        }
        crate::log::log(&format!(
            "matcher: prewarmed a-z in {} ms",
            started.elapsed().as_millis()
        ));
    });
}

fn cache() -> MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(Cache::new(DEFAULT_CACHE_SIZE)))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Compiled queries keyed by their exact text. `prewarmed` is never
/// evicted; `recent` holds at most `capacity` entries and evicts the least
/// recently used one when full.
struct Cache {
    capacity: usize,
    clock: u64,
    recent: HashMap<String, (CompiledQuery, u64)>,
    prewarmed: HashMap<String, CompiledQuery>,
}

impl Cache {
    fn new(capacity: usize) -> Self {
        Cache { capacity, clock: 0, recent: HashMap::new(), prewarmed: HashMap::new() }
    }

    fn get(&mut self, key: &str) -> Option<CompiledQuery> {
        if let Some(compiled) = self.prewarmed.get(key) {
            return Some(compiled.clone());
        }
        self.clock += 1;
        let clock = self.clock;
        self.recent.get_mut(key).map(|(compiled, last_used)| {
            *last_used = clock;
            compiled.clone()
        })
    }

    fn insert_recent(&mut self, key: String, compiled: CompiledQuery) {
        if self.capacity == 0 || self.prewarmed.contains_key(&key) {
            return;
        }
        if !self.recent.contains_key(&key) && self.recent.len() >= self.capacity {
            // A linear scan is fine: it only runs on a cache miss, which
            // just paid for a compile that costs far more.
            let oldest = self
                .recent
                .iter()
                .min_by_key(|(_, (_, last_used))| *last_used)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.recent.remove(&oldest);
            }
        }
        self.clock += 1;
        self.recent.insert(key, (compiled, self.clock));
    }

    fn insert_prewarmed(&mut self, key: String, compiled: CompiledQuery) {
        self.recent.remove(&key);
        self.prewarmed.insert(key, compiled);
    }

    #[cfg(test)]
    fn contains(&self, key: &str) -> bool {
        self.prewarmed.contains_key(key) || self.recent.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str, friendly_name: &str) -> TaskWindow {
        TaskWindow {
            hwnd: std::ptr::null_mut(),
            title: title.to_string(),
            exe_path: String::new(),
            friendly_name: friendly_name.to_string(),
            minimized: false,
        }
    }

    #[test]
    fn blank_query_matches_everything() {
        assert!(compile("").matches(&window("Steam", "Steam")));
        assert!(compile("   ").matches(&window("Steam", "Steam")));
    }

    #[test]
    fn romaji_matches_japanese() {
        let memo = build("memo");
        assert!(memo.matches(&window("無題 - メモ帳", "メモ帳")));
        assert!(!memo.matches(&window("Steam", "Steam")));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(build("claude").matches(&window("Claude", "Claude")));
        assert!(build("CLAUDE").matches(&window("claude", "claude")));
    }

    #[test]
    fn friendly_name_alone_can_match() {
        assert!(build("chrome").matches(&window("GitHub - oguna/rustmigemo", "Google Chrome")));
    }

    #[test]
    fn multiple_words_require_all_to_match_and_ignore_order() {
        let w = window("2) afx - Notepad", "Notepad");
        assert!(compile("afx 2").matches(&w));
        assert!(compile("2 afx").matches(&w));
        assert!(!compile("afx 3").matches(&w));
    }

    #[test]
    fn multiple_words_can_each_match_a_different_field() {
        let w = window("afx", "Chrome");
        assert!(compile("afx chrome").matches(&w));
    }

    #[test]
    fn extra_whitespace_between_words_is_ignored() {
        let w = window("2) afx - Notepad", "Notepad");
        assert!(compile("  afx   2  ").matches(&w));
    }

    #[test]
    fn cache_evicts_the_least_recently_used_entry() {
        let mut cache = Cache::new(2);
        cache.insert_recent("a".into(), CompiledQuery::All);
        cache.insert_recent("b".into(), CompiledQuery::All);
        assert!(cache.get("a").is_some()); // "b" is now the oldest
        cache.insert_recent("c".into(), CompiledQuery::All);
        assert!(cache.contains("a"));
        assert!(!cache.contains("b"));
        assert!(cache.contains("c"));
    }

    #[test]
    fn reinserting_an_existing_key_does_not_evict() {
        let mut cache = Cache::new(2);
        cache.insert_recent("a".into(), CompiledQuery::All);
        cache.insert_recent("b".into(), CompiledQuery::All);
        cache.insert_recent("b".into(), CompiledQuery::All);
        assert!(cache.contains("a") && cache.contains("b"));
    }

    #[test]
    fn zero_capacity_stores_nothing() {
        let mut cache = Cache::new(0);
        cache.insert_recent("a".into(), CompiledQuery::All);
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn prewarmed_entries_survive_a_full_cache_and_a_disabled_one() {
        let mut cache = Cache::new(0);
        cache.insert_prewarmed("k".into(), CompiledQuery::All);
        assert!(cache.get("k").is_some());

        let mut cache = Cache::new(1);
        cache.insert_recent("k".into(), CompiledQuery::All);
        cache.insert_prewarmed("k".into(), CompiledQuery::All);
        cache.insert_recent("x".into(), CompiledQuery::All);
        cache.insert_recent("y".into(), CompiledQuery::All);
        assert!(cache.contains("k"));
        assert_eq!(cache.recent.len(), 1);
    }

    /// Timing, not correctness, so it only runs on request -- and in a
    /// release build, where the numbers mean something:
    /// `cargo test --release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cache_hit_skips_the_expensive_build() {
        let query_text = "k"; // single letters are the worst case
        let started = Instant::now();
        let _ = compile(query_text);
        let miss = started.elapsed();
        let started = Instant::now();
        let _ = compile(query_text);
        let hit = started.elapsed();
        println!("compile(\"{}\"): miss {:?}, hit {:?}", query_text, miss, hit);
        assert!(hit * 100 < miss, "a cache hit should be far cheaper than a build");
    }

    /// Runs the real background thread, so also opt-in:
    /// `cargo test --release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn prewarm_fills_all_single_letters() {
        let started = Instant::now();
        prewarm_in_background();
        while cache().prewarmed.len() < 26 {
            assert!(started.elapsed().as_secs() < 60, "prewarm did not finish within 60 s");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!("prewarm finished in {:?}", started.elapsed());
        let started = Instant::now();
        let _ = compile("c");
        println!("compile(\"c\") after prewarm: {:?}", started.elapsed());
        assert!(started.elapsed().as_millis() < 5);
    }
}
