//! URLs said in a transcript, written ("github.com/coderscantina/shot-studio") or spoken
//! ("github dot com slash ..."). The same reading as the link card rule of milestone 07, which
//! moves here with the rule becoming a style rule.

use shot_core::{TimeRange, Word};

/// A URL said and the words that say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaidUrl {
    pub url: String,
    /// `github`, `x`, `news.ycombinator`, `npm`, or the host name.
    pub host: String,
    pub words: TimeRange,
}

/// Spoken punctuation in URLs and what it writes.
const SPOKEN: [(&[&str], &str); 7] = [
    (&["dot"], "."),
    (&["slash"], "/"),
    (&["dash"], "-"),
    (&["hyphen"], "-"),
    (&["question", "mark"], "?"),
    (&["equals"], "="),
    (&["colon"], ":"),
];

/// Hosts a spoken URL may start with, so "go dot dev" is not taken for a link.
const HOSTS: [&str; 7] = [
    "github.com",
    "x.com",
    "twitter.com",
    "news.ycombinator.com",
    "npmjs.com",
    "www.npmjs.com",
    "www.github.com",
];

/// Every URL said in `words`, in order. A written URL is any word that reads as a link with a
/// dotted host; a spoken one must start with a known host.
pub fn urls(words: &[Word]) -> Vec<SaidUrl> {
    let tokens: Vec<String> = words.iter().map(|w| token(&w.text)).collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut text = String::new();
        let mut end = i;
        let mut j = i;
        // Joins words and spoken punctuation for as long as they read as one link.
        while j < words.len() {
            if let Some((len, symbol)) = spoken_at(&tokens, j) {
                if text.is_empty() {
                    break;
                }
                text.push_str(symbol);
                j += len;
                continue;
            }
            let word = tokens[j].as_str();
            let joins = text.is_empty() || text.ends_with(['.', '/', '-', '?', '=', ':']);
            if !joins || word.is_empty() {
                break;
            }
            text.push_str(word);
            j += 1;
            end = j;
        }
        let text = text.trim_end_matches(['.', '/', '-', '?', '=', ':']);
        let url = if text.starts_with("http") {
            text.to_owned()
        } else {
            format!("https://{text}")
        };
        let spoken = end > i + 1;
        let said = end > i && text.contains('.');
        match host(&url).filter(|(name, _)| said && (!spoken || HOSTS.contains(&name.as_str()))) {
            Some((_, host)) => {
                if let Some(range) =
                    TimeRange::new(words[i].range.from(), words[end - 1].range.to())
                {
                    found.push(SaidUrl {
                        url,
                        host,
                        words: range,
                    });
                }
                i = end;
            }
            None => i += 1,
        }
    }
    found
}

/// The host name of an http(s) link and its short name for `host=`, `None` when it has no
/// dotted host.
fn host(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let name = rest.split('/').next().unwrap_or(rest).to_lowercase();
    let bare = name.strip_prefix("www.").unwrap_or(&name);
    if !bare.contains('.') || bare.starts_with('.') || bare.ends_with('.') {
        return None;
    }
    let short = match bare {
        "github.com" => "github",
        "x.com" | "twitter.com" => "x",
        "news.ycombinator.com" => "news.ycombinator",
        "npmjs.com" => "npm",
        other => other,
    }
    .to_owned();
    Some((name, short))
}

/// A word as it would be written in a URL: lowercase, without trailing sentence punctuation.
fn token(text: &str) -> String {
    text.trim_matches(|c: char| matches!(c, ',' | '"' | '\'' | '(' | ')' | '!'))
        .trim_end_matches(['.', '?', ':', ';'])
        .to_lowercase()
}

/// Spoken punctuation starting at word `i`: how many words it takes and what it writes.
fn spoken_at(tokens: &[String], i: usize) -> Option<(usize, &'static str)> {
    SPOKEN.iter().find_map(|(spoken, symbol)| {
        let fits = tokens.len() >= i + spoken.len()
            && spoken.iter().zip(&tokens[i..]).all(|(a, b)| a == b);
        fits.then_some((spoken.len(), *symbol))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Micros;

    fn words(text: &str) -> Vec<Word> {
        text.split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.to_owned(),
                range: TimeRange::new(
                    Micros(i as u64 * 400_000),
                    Micros(i as u64 * 400_000 + 300_000),
                )
                .unwrap(),
                confidence: 90,
            })
            .collect()
    }

    #[test]
    fn finds_written_and_spoken_urls_with_their_host() {
        let found = urls(&words(
            "See github.com/coderscantina/shot-studio/pull/1, then github dot com slash zed, not go dot dev, and blog.example.org.",
        ));
        let got: Vec<(&str, &str)> = found
            .iter()
            .map(|u| (u.url.as_str(), u.host.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "https://github.com/coderscantina/shot-studio/pull/1",
                    "github"
                ),
                ("https://github.com/zed", "github"),
                ("https://blog.example.org", "blog.example.org"),
            ]
        );
        assert_eq!(found[1].words.from(), Micros(3 * 400_000));
    }
}
