//! Parsers on broken files: every prefix and every line-deleted variant of the contract's
//! `cut.kdl` and `format.kdl` examples parses or is refused, within a time budget.

mod common;

use std::time::Duration;

use common::within;
use shot_core::{Cut, FormatLayer};

const CUT: &str = include_str!("../../../fixtures/examples/cut.kdl");
const FORMAT: &str = include_str!("../../../fixtures/examples/format.kdl");
const BUDGET: Duration = Duration::from_secs(5);

/// Truncated at every character, then with each line missing.
fn broken(text: &str) -> Vec<(String, String)> {
    let prefixes = text
        .char_indices()
        .map(|(end, _)| (format!("the first {end} bytes"), text[..end].to_owned()));
    let lines: Vec<&str> = text.lines().collect();
    let without = (0..lines.len()).map(|skip| {
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| (i != skip).then_some(*l))
            .collect();
        (format!("line {} removed", skip + 1), kept.join("\n"))
    });
    prefixes.chain(without).collect()
}

#[test]
fn broken_cut_and_format_files_are_parsed_or_refused_in_time() {
    for (name, text, parse) in [
        (
            "cut.kdl",
            CUT,
            (|t: &str| Cut::parse(t).is_ok()) as fn(&str) -> bool,
        ),
        ("format.kdl", FORMAT, |t| FormatLayer::parse(t).is_ok()),
    ] {
        for (what, variant) in broken(text) {
            let input = format!("{name}, {what}");
            let copy = variant.clone();
            within(BUDGET, &input, move || parse(&copy));
        }
    }
}

#[test]
fn a_hang_fails_the_test_and_names_its_input() {
    let hang = std::panic::catch_unwind(|| {
        within(
            Duration::from_millis(50),
            "cut.kdl, the first 7 bytes",
            || {
                std::thread::sleep(Duration::from_secs(2));
            },
        );
    })
    .unwrap_err();
    let message = hang.downcast_ref::<String>().unwrap();
    assert!(message.contains("cut.kdl, the first 7 bytes"), "{message}");
}
