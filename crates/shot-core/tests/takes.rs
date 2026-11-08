//! The `takes` block of `cut.kdl`: ratings and picks of the Takes view.

use shot_core::{Attempt, Cut, Id, Micros, TimeRange};

const CUT: &str = include_str!("../../../fixtures/examples/takes/cut.kdl");
const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

#[test]
fn the_takes_example_round_trips_and_is_in_the_contract() {
    assert!(
        CONTRACT.contains(CUT),
        "the contract is missing this example:\n{CUT}"
    );
    let cut = Cut::parse(CUT).unwrap();
    assert_eq!(cut.to_kdl(), CUT);
    assert_eq!(cut.ratings.len(), 4);
    // The second take's second attempt ran 13.9 s to 19.5 s; its circled part binds to it.
    let attempt = Attempt {
        take: Id::new("t02").unwrap(),
        range: TimeRange::new(Micros(13_900_000), Micros(19_500_000)).unwrap(),
    };
    let rating = cut.rating(&attempt).unwrap();
    assert!(rating.pick);
    assert_eq!(rating.stars, None);
}

#[test]
fn attempts_are_sorted_and_checked() {
    let unsorted =
        "takes {\n    attempt t10 from=0 to=5 stars=1\n    attempt t9 from=0 to=5 stars=2\n}\n";
    assert_eq!(
        Cut::parse(unsorted).unwrap().to_kdl(),
        "takes {\n    attempt t9 from=0 to=5 stars=2\n    attempt t10 from=0 to=5 stars=1\n}\n"
    );
    for (text, message) in [
        (
            "takes {\n    attempt t01 from=0 to=5 stars=6\n}\n",
            "`stars` is 1 to 5",
        ),
        (
            "takes {\n    attempt t01 from=0 to=5\n}\n",
            "an `attempt` has `stars`, `pick=#true` or both",
        ),
        (
            "takes {\n    attempt t01 from=0 to=5 stars=1\n    attempt t01 from=4 to=9 stars=2\n}\n",
            "`attempt` ranges of take `t01` must not overlap",
        ),
    ] {
        let error = Cut::parse(text).unwrap_err();
        assert_eq!(error.message, message, "{text}");
    }
}
