//! `NSSpellChecker`. AppKit's header documents one background path, `requestCheckingOfString:...`:
//! it takes the request, does the work elsewhere, and calls the completion handler "in an arbitrary
//! context". It says nothing about calling the synchronous methods off the main thread, so the
//! requests, the suggestions and learning a word happen on the main thread, where they cost little,
//! and all checking happens on AppKit's own background queue. The checker finds the language of the
//! text itself, as the person's system settings have it.

use std::cell::Cell;
use std::ops::Range;
use std::sync::Mutex;

use block2::RcBlock;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{
    NSArray, NSOrthography, NSRange, NSString, NSTextCheckingResult, NSTextCheckingTypes,
};

use super::{Checker, Done};

/// `NSTextCheckingTypeSpelling`.
const SPELLING: NSTextCheckingTypes = 1 << 1;

#[derive(Default)]
pub struct SystemChecker {
    tag: Cell<Option<isize>>,
}

impl SystemChecker {
    /// One document tag per checker, so the checker keeps nothing between unrelated requests of
    /// ours.
    fn tag(&self) -> isize {
        self.tag.get().unwrap_or_else(|| {
            let tag = NSSpellChecker::uniqueSpellDocumentTag();
            self.tag.set(Some(tag));
            tag
        })
    }
}

impl Checker for SystemChecker {
    fn check(&self, text: String, done: Done) {
        let checker = NSSpellChecker::sharedSpellChecker();
        let string = NSString::from_str(&text);
        let range = NSRange::new(0, string.length());
        // The handler is a `Fn`, and AppKit calls it once.
        let done = Mutex::new(Some((done, text)));
        let handler = RcBlock::new(
            move |_: isize,
                  results: std::ptr::NonNull<NSArray<NSTextCheckingResult>>,
                  _: std::ptr::NonNull<NSOrthography>,
                  _: isize| {
                let Some((done, text)) = done.lock().ok().and_then(|mut done| done.take()) else {
                    return;
                };
                // SAFETY: AppKit passes a valid array for the duration of the call.
                let results = unsafe { results.as_ref() };
                let ranges: Vec<NSRange> = results.iter().map(|result| result.range()).collect();
                done(byte_ranges(&text, ranges));
            },
        );
        // SAFETY: no options, so there is no generic to get wrong.
        unsafe {
            checker.requestCheckingOfString_range_types_options_inSpellDocumentWithTag_completionHandler(
                &string,
                range,
                SPELLING,
                None,
                self.tag(),
                Some(&handler),
            );
        }
    }

    fn suggestions(&self, word: &str) -> Vec<String> {
        let string = NSString::from_str(word);
        let range = NSRange::new(0, string.length());
        NSSpellChecker::sharedSpellChecker()
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                range,
                &string,
                None,
                self.tag(),
            )
            .map(|guesses| guesses.iter().map(|guess| guess.to_string()).collect())
            .unwrap_or_default()
    }

    fn learn(&self, word: &str) {
        NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
    }
}

/// Turns the checker's UTF-16 ranges, in rising order, into byte ranges of `text`.
fn byte_ranges(text: &str, ranges: Vec<NSRange>) -> Vec<Range<usize>> {
    let mut found = Vec::with_capacity(ranges.len());
    let mut chars = text.char_indices();
    let (mut utf16, mut byte) = (0, 0);
    let mut advance_to = |target: usize| {
        while utf16 < target {
            match chars.next() {
                Some((at, character)) => {
                    utf16 += character.len_utf16();
                    byte = at + character.len_utf8();
                }
                None => break,
            }
        }
        byte
    };
    for range in ranges {
        let start = advance_to(range.location);
        let end = advance_to(range.location + range.length);
        found.push(start..end);
    }
    found
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_typo_is_found_off_the_main_thread_and_has_suggestions() {
        let text = "The house and teh function.";
        let (tx, rx) = mpsc::channel();
        let checker = SystemChecker::default();
        checker.check(
            text.to_owned(),
            Box::new(move |ranges| {
                tx.send(ranges).ok();
            }),
        );
        let ranges = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the checker answers");
        let found: Vec<&str> = ranges.into_iter().map(|range| &text[range]).collect();
        assert_eq!(found, ["teh"]);
        assert!(
            checker
                .suggestions("teh")
                .iter()
                .any(|guess| guess == "the")
        );
    }

    #[test]
    fn utf16_ranges_become_byte_ranges_past_umlauts_and_emoji() {
        let text = "Größe 😀 Fehhler";
        let ranges = byte_ranges(text, vec![NSRange::new(0, 5), NSRange::new(9, 7)]);
        assert_eq!(&text[ranges[0].clone()], "Größe");
        assert_eq!(&text[ranges[1].clone()], "Fehhler");
    }
}
