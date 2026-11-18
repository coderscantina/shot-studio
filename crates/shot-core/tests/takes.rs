//! The `takes` block of `cut.kdl`: a person's picks among the attempts.

use shot_core::{Cut, Id, Micros, TimeRange};

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
    assert_eq!(cut.picks.len(), 2);
    assert_eq!(cut.picks[1].take, Id::new("t02").unwrap());
    assert_eq!(
        cut.picks[1].range,
        TimeRange::new(Micros(14_100_000), Micros(18_200_000)).unwrap()
    );
}

#[test]
fn picks_are_sorted_and_checked_and_old_stars_are_dropped() {
    let old = "takes {\n    attempt t10 from=0 to=5 stars=1 pick=#true\n    attempt t9 from=0 to=5 pick=#true\n    attempt t9 from=6 to=9 stars=4\n}\n";
    assert_eq!(
        Cut::parse(old).unwrap().to_kdl(),
        "takes {\n    attempt t9 from=0 to=5 pick=#true\n    attempt t10 from=0 to=5 pick=#true\n}\n"
    );
    for (text, message) in [
        (
            "takes {\n    attempt t01 from=0 to=5\n}\n",
            "an `attempt` has `pick=#true`",
        ),
        (
            "takes {\n    attempt t01 from=0 to=5 pick=#true\n    attempt t01 from=4 to=9 pick=#true\n}\n",
            "`attempt` ranges of take `t01` must not overlap",
        ),
    ] {
        let error = Cut::parse(text).unwrap_err();
        assert_eq!(error.message, message, "{text}");
    }
}
