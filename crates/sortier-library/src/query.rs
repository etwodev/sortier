//! Search query language.
//!
//! ```text
//! cat blue_sky          both tags (AND)
//! -sketch               exclude a tag
//! artist:foo*           glob (* and ?)
//! cat|dog               either tag (OR)
//! type:video            image | gif | video   (also is:video, is:gif)
//! is:untagged           no tags at all       (is:tagged, is:seq, is:single)
//! name:draft            file name contains
//! seq:chapter           sequence name contains
//! col:series            collection name contains   (is:collection)
//! ext:png               file extension
//! ```

use crate::library::{Collection, Group, Item, MediaKind};

#[derive(Debug, Clone)]
enum Pat {
    Exact(String),
    Glob(String),
}

impl Pat {
    fn new(s: &str) -> Self {
        if s.contains(['*', '?']) {
            Pat::Glob(s.to_string())
        } else {
            Pat::Exact(s.to_string())
        }
    }
    fn matches(&self, tag: &str) -> bool {
        match self {
            Pat::Exact(s) => s == tag,
            Pat::Glob(g) => glob_match(g.as_bytes(), tag.as_bytes()),
        }
    }
}

fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while i < s.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == s[i]) {
            p += 1;
            i += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = Some(p);
            mark = i;
            p += 1;
        } else if let Some(sp) = star {
            p = sp + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

#[derive(Debug, Clone)]
enum Matcher {
    Tags(Vec<Pat>),
    Kind(MediaKind),
    Untagged,
    Grouped,
    Name(String),
    Ext(String),
    Seq(String),
    Collected,
    Col(String),
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    terms: Vec<(bool, Matcher)>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let mut terms = Vec::new();
        for raw in text.split_whitespace() {
            let (neg, tok) = match raw.strip_prefix('-').or_else(|| raw.strip_prefix('!')) {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            let tok = tok.to_lowercase();
            if tok.is_empty() {
                continue;
            }
            let m = if let Some(v) = tok.strip_prefix("type:").or_else(|| tok.strip_prefix("is:")) {
                match v {
                    "video" | "videos" => Matcher::Kind(MediaKind::Video),
                    "gif" | "animated" => Matcher::Kind(MediaKind::Animated),
                    "image" | "images" | "still" => Matcher::Kind(MediaKind::Image),
                    "doc" | "docs" | "document" | "documents" | "book" | "books" => Matcher::Kind(MediaKind::Document),
                    "untagged" => Matcher::Untagged,
                    "tagged" => {
                        terms.push((!neg, Matcher::Untagged));
                        continue;
                    }
                    "seq" | "sequence" | "grouped" => Matcher::Grouped,
                    "col" | "collection" | "collected" => Matcher::Collected,
                    "single" | "loose" => {
                        terms.push((!neg, Matcher::Grouped));
                        continue;
                    }
                    _ => Matcher::Tags(vec![Pat::new(&tok)]),
                }
            } else if let Some(v) = tok.strip_prefix("name:") {
                Matcher::Name(v.to_string())
            } else if let Some(v) = tok.strip_prefix("ext:") {
                Matcher::Ext(v.trim_start_matches('.').to_string())
            } else if let Some(v) = tok.strip_prefix("seq:") {
                Matcher::Seq(v.replace('_', " "))
            } else if let Some(v) = tok.strip_prefix("col:") {
                Matcher::Col(v.replace('_', " "))
            } else {
                Matcher::Tags(tok.split('|').filter(|s| !s.is_empty()).map(Pat::new).collect())
            };
            terms.push((neg, m));
        }
        Query { terms }
    }

    /// Does the item (with its sequence's and collection's tags folded in) satisfy every term?
    pub fn matches(&self, item: &Item, group: Option<&Group>, collection: Option<&Collection>) -> bool {
        self.terms.iter().all(|(neg, m)| m.hits(item, group, collection) != *neg)
    }
}

impl Matcher {
    fn hits(&self, item: &Item, group: Option<&Group>, collection: Option<&Collection>) -> bool {
        let mut tags = item
            .tags
            .iter()
            .chain(group.into_iter().flat_map(|g| g.tags.iter()))
            .chain(collection.into_iter().flat_map(|c| c.tags.iter()));
        match self {
            Matcher::Tags(pats) => tags.any(|t| pats.iter().any(|p| p.matches(t))),
            Matcher::Kind(k) => item.kind == *k,
            Matcher::Untagged => tags.next().is_none(),
            Matcher::Grouped => group.is_some(),
            Matcher::Name(s) => item.file_name().to_lowercase().contains(s.as_str()),
            Matcher::Ext(e) => item
                .path
                .extension()
                .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(e)),
            Matcher::Seq(s) => group.is_some_and(|g| g.name.to_lowercase().contains(s.as_str())),
            Matcher::Collected => collection.is_some(),
            Matcher::Col(s) => collection.is_some_and(|c| c.name.to_lowercase().contains(s.as_str())),
        }
    }
}

/// Toggle a tag in a query string. `exclude` selects `-tag` instead of `tag`.
/// Clicking the same state again removes it; clicking the opposite state flips it.
pub fn toggle_tag_in_query(query: &str, tag: &str, exclude: bool) -> String {
    let want = if exclude { format!("-{tag}") } else { tag.to_string() };
    let opposite = if exclude { tag.to_string() } else { format!("-{tag}") };
    let mut toks: Vec<String> = query.split_whitespace().map(str::to_string).collect();
    if let Some(pos) = toks.iter().position(|t| t.eq_ignore_ascii_case(&want)) {
        toks.remove(pos);
    } else if let Some(pos) = toks.iter().position(|t| t.eq_ignore_ascii_case(&opposite)) {
        toks[pos] = want;
    } else {
        toks.push(want);
    }
    toks.join(" ")
}

/// Is the tag currently included (`Some(false)`) or excluded (`Some(true)`) in the query?
pub fn tag_state_in_query(query: &str, tag: &str) -> Option<bool> {
    for t in query.split_whitespace() {
        if t.eq_ignore_ascii_case(tag) {
            return Some(false);
        }
        if let Some(rest) = t.strip_prefix('-')
            && rest.eq_ignore_ascii_case(tag) {
                return Some(true);
            }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn item(tags: &[&str], kind: MediaKind) -> Item {
        Item {
            id: 1,
            path: PathBuf::from("C:/art/Draft_01.png"),
            kind,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            group: None,
            size: 0,
            modified: 0,
            added: 0,
            dims: None,
            pages: None,
            cover: None,
        }
    }

    #[test]
    fn include_exclude() {
        let it = item(&["cat", "artist:foo"], MediaKind::Image);
        assert!(Query::parse("cat").matches(&it, None, None));
        assert!(!Query::parse("cat -artist:foo").matches(&it, None, None));
        assert!(Query::parse("artist:f*").matches(&it, None, None));
        assert!(Query::parse("dog|cat").matches(&it, None, None));
        assert!(!Query::parse("dog").matches(&it, None, None));
        assert!(Query::parse("-dog type:image").matches(&it, None, None));
        assert!(!Query::parse("is:video").matches(&it, None, None));
        assert!(Query::parse("name:draft ext:png").matches(&it, None, None));
        assert!(Query::parse("is:tagged").matches(&it, None, None));
        assert!(!Query::parse("is:untagged").matches(&it, None, None));
    }

    #[test]
    fn group_tags_count() {
        let it = item(&[], MediaKind::Image);
        let g = Group {
            id: 2,
            name: "Chapter One".into(),
            items: vec![1],
            tags: BTreeSet::from(["artist:bar".to_string()]),
            auto_key: None,
            cover: None,
        };
        assert!(Query::parse("artist:bar").matches(&it, Some(&g), None));
        assert!(Query::parse("seq:chapter_one is:seq").matches(&it, Some(&g), None));
        assert!(!Query::parse("-artist:bar").matches(&it, Some(&g), None));
        assert!(!Query::parse("is:untagged").matches(&it, Some(&g), None));
    }

    #[test]
    fn toggling() {
        assert_eq!(toggle_tag_in_query("a b", "c", false), "a b c");
        assert_eq!(toggle_tag_in_query("a b c", "c", false), "a b");
        assert_eq!(toggle_tag_in_query("a c", "c", true), "a -c");
        assert_eq!(tag_state_in_query("a -c", "c"), Some(true));
    }
}
