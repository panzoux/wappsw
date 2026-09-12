use std::sync::OnceLock;

use regex::RegexBuilder;
use rustmigemo::migemo::compact_dictionary::CompactDictionary;
use rustmigemo::migemo::query::query;
use rustmigemo::migemo::regex_generator::RegexOperator;

use crate::window_list::TaskWindow;

// BSD-derived (Mozc + UniDic), see assets/LICENSE*.
static DICT_BYTES: &[u8] = include_bytes!("../assets/migemo-compact-dict.bin");

fn dictionary() -> &'static CompactDictionary {
    static DICT: OnceLock<CompactDictionary> = OnceLock::new();
    DICT.get_or_init(|| CompactDictionary::new(&DICT_BYTES.to_vec()))
}

/// Compiles `query_text` into a migemo regex (romaji -> kana/kanji reading
/// expansion) and tests it against `haystack`. Case-insensitive: migemo's
/// own query() keeps the literal query text in whatever case was typed
/// (e.g. "c"), and Japanese has no case anyway, so a case-sensitive compile
/// would fail to match e.g. "Claude" against a lowercase "c" query. Falls
/// back to a plain case-insensitive substring check if the generated
/// pattern fails to compile.
fn regex_matches(query_text: &str, haystack: &str) -> bool {
    let pattern = query(query_text.to_string(), dictionary(), &RegexOperator::Default);
    if pattern.is_empty() {
        return true;
    }
    match RegexBuilder::new(&pattern).case_insensitive(true).build() {
        Ok(re) => re.is_match(haystack),
        Err(e) => {
            crate::log::log(&format!("matcher: generated pattern failed to compile: {}", e));
            haystack.to_lowercase().contains(&query_text.to_lowercase())
        }
    }
}

/// A window matches if the query matches its title or its resolved friendly
/// app name.
pub fn window_matches(query_text: &str, window: &TaskWindow) -> bool {
    if query_text.trim().is_empty() {
        return true;
    }
    regex_matches(query_text, &window.title) || regex_matches(query_text, &window.friendly_name)
}
