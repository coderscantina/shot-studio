use std::io::Write;

use super::*;

/// Words of 0.3 s back to back from 1 s, `|` a 1.5 s gap.
fn transcript(text: &str) -> Transcript {
    let mut transcript = Transcript::new(Id::new("t01").unwrap());
    let mut at = 1_000_000;
    for token in text.split(' ') {
        if token == "|" {
            at += 1_500_000;
            continue;
        }
        transcript
            .push(Word {
                text: token.to_owned(),
                range: TimeRange::new(Micros(at), Micros(at + 300_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
        at += 300_000;
    }
    transcript
}

fn lines(elements: &Elements, kinds: &[ElementKind]) -> Vec<String> {
    elements
        .to_jsonl()
        .lines()
        .filter(|line| {
            kinds
                .iter()
                .any(|k| line.starts_with(&format!("{{\"kind\":\"{k}\"")))
        })
        .map(str::to_owned)
        .collect()
}

fn facts<'a>(
    transcript: &'a Transcript,
    speech: &'a Speech,
    markers: &'a [Marker],
    input: &'a Input,
    script: Option<&'a Script>,
) -> Facts<'a> {
    Facts {
        transcript,
        speech,
        duration: Micros(20_000_000),
        still: &[],
        markers,
        input,
        script,
    }
}

#[test]
fn the_transcript_gives_words_sentences_whole_gaps_fillers_retakes_and_urls() {
    let t = transcript("So, um, we ship | it. | See github.com/zed-industries/zed the the end.");
    let speech = Speech::default();
    let input = Input::default();
    let elements = local(&facts(&t, &speech, &[], &input, None));
    use ElementKind::*;
    assert_eq!(
        lines(&elements, &[Sentence, Silence, Pause, Filler, Retake, Url]),
        [
            r#"{"kind":"silence","t":0,"to":1000000,"confidence":95,"by":"vad@silero-6.2"}"#,
            r#"{"kind":"sentence","t":1000000,"to":4000000,"by":"whisper@large-v3-turbo"}"#,
            r#"{"kind":"filler","t":1300000,"to":1600000,"confidence":92,"by":"whisper@large-v3-turbo","text":"um"}"#,
            r#"{"kind":"pause","t":2200000,"to":3700000,"confidence":75,"by":"vad@silero-6.2"}"#,
            r#"{"kind":"silence","t":4000000,"to":5500000,"confidence":95,"by":"vad@silero-6.2"}"#,
            r#"{"kind":"sentence","t":5500000,"to":7000000,"by":"whisper@large-v3-turbo"}"#,
            r#"{"kind":"url","t":5800000,"to":6100000,"by":"transcript","url":"https://github.com/zed-industries/zed","host":"github"}"#,
            r#"{"kind":"retake","t":6100000,"to":6400000,"confidence":85,"by":"whisper@large-v3-turbo"}"#,
            r#"{"kind":"silence","t":7000000,"to":20000000,"confidence":95,"by":"vad@silero-6.2"}"#,
        ]
    );
    assert_eq!(elements.of_kind(Word).count(), 10);
}

#[test]
fn the_event_log_and_the_prompter_give_clicks_typing_markers_beats_chapters_and_tags() {
    let script = Script::parse(
        "# T\n\n## Hook\n\n- Say it {hook}\n\n## Demo\n\n- Show the diff {diff} {code-1}\n- Wrap up\n",
    )
    .unwrap();
    let marker = |t: u64, mark| Marker {
        t: Micros(t),
        x: 10,
        y: 20,
        mark,
    };
    let markers = [
        marker(4_000_000, Mark::Next { beat: 2 }),
        marker(5_000_000, Mark::Zoom),
        marker(9_000_000, Mark::Again { beat: 2 }),
        marker(12_000_000, Mark::Next { beat: 3 }),
    ];
    let input = Input {
        clicks: vec![(Micros(2_000_000), 2210, 640)],
        keys: [6.0, 6.2, 6.5, 8.0, 8.1]
            .map(|s: f64| Micros((s * 1e6) as u64))
            .to_vec(),
    };
    let t = Transcript::new(Id::new("t01").unwrap());
    let speech = Speech::default();
    let elements = local(&facts(&t, &speech, &markers, &input, Some(&script)));
    use ElementKind::{Beat, Chapter, Click, Tag, Typing};
    assert_eq!(
        lines(
            &elements,
            &[Click, Typing, ElementKind::Marker, Beat, Chapter, Tag]
        ),
        [
            r#"{"kind":"chapter","t":0,"by":"script","text":"Hook"}"#,
            r#"{"kind":"beat","t":0,"by":"script","index":1}"#,
            r#"{"kind":"tag","t":0,"to":4000000,"by":"script","name":"hook"}"#,
            r#"{"kind":"click","t":2000000,"by":"events","x":2210,"y":640}"#,
            r#"{"kind":"chapter","t":4000000,"by":"script","text":"Demo"}"#,
            r#"{"kind":"beat","t":4000000,"by":"script","index":2}"#,
            r#"{"kind":"tag","t":4000000,"to":9000000,"by":"script","name":"diff"}"#,
            r#"{"kind":"marker","t":5000000,"by":"events","label":"Z"}"#,
            r#"{"kind":"typing","t":6000000,"to":6500000,"by":"events"}"#,
            r#"{"kind":"chapter","t":9000000,"by":"script","text":"Demo"}"#,
            r#"{"kind":"beat","t":9000000,"by":"script","index":2}"#,
            r#"{"kind":"tag","t":9000000,"to":12000000,"by":"script","name":"diff"}"#,
            r#"{"kind":"beat","t":12000000,"by":"script","index":3}"#,
        ]
    );
    let without_prompter = local(&facts(&t, &speech, &markers[1..2], &input, Some(&script)));
    assert_eq!(without_prompter.of_kind(Beat).count(), 0);
}

#[test]
fn the_stream_writes_what_the_quick_pass_writes_for_the_same_facts_as_they_arrive() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let take = Id::new("t01").unwrap();
    let mut stream = Stream::create(video, &take).unwrap();
    assert!(read(video, &take).unwrap().unwrap().is_empty());
    let media = video.join("media/t01");
    fs::create_dir_all(&media).unwrap();
    let events = media.join("events.jsonl");
    let mut log = fs::File::create(&events).unwrap();
    let appended = [
        "{\"t\":500000,\"kind\":\"move\",\"x\":5,\"y\":6}\n",
        "{\"t\":2000000,\"kind\":\"down\",\"x\":1,\"y\":2,\"button\":\"left\"}\n",
        "{\"t\":3000000,\"kind\":\"key\",\"key\":\"a\"}\n{\"t\":3200000,\"kind\":\"key\",\"key\":\"b\"}\n",
        "{\"t\":3400000,\"kind\":\"key\",\"key\":\"c\"}\n{\"t\":5000000,\"kind\":\"marker\",\"x\":1,\"y\":1,\"mark\":\"zoom\"}\n",
    ];
    // Capture writes the marker line in two parts; the first part is no line yet.
    let (head, tail) = appended[3].split_at(appended[3].len() - 20);
    log.write_all(appended[0].as_bytes()).unwrap();
    assert_eq!(stream.poll().unwrap(), 1);
    assert!(!stream.update().unwrap(), "a cursor move is no element");
    for part in [appended[1], appended[2], head] {
        log.write_all(part.as_bytes()).unwrap();
    }
    stream.poll().unwrap();
    assert!(stream.update().unwrap());
    assert_eq!(stream.live().of_kind(ElementKind::Typing).count(), 1);
    assert_eq!(stream.live().of_kind(ElementKind::Marker).count(), 0);
    log.write_all(tail.as_bytes()).unwrap();
    assert_eq!(stream.poll().unwrap(), 1);
    assert!(stream.update().unwrap());

    let t = transcript("See github dot com slash zed | now.");
    let (said, rest) = t.words().split_at(3);
    for word in said {
        stream.word(word.clone());
    }
    stream.tentative(rest.to_vec());
    stream.update().unwrap();
    let url = |e: &Elements| -> Vec<String> {
        e.of_kind(ElementKind::Url)
            .filter_map(|u| u.text(Attr::Url).map(str::to_owned))
            .collect()
    };
    assert_eq!(url(stream.live()), ["https://github.com/zed"]);
    assert!(
        url(&read(video, &take).unwrap().unwrap()).is_empty(),
        "tentative words stay out of the file"
    );
    for word in rest {
        stream.word(word.clone());
    }
    stream.tentative(Vec::new());
    stream.update().unwrap();
    assert_eq!(stream.markers().len(), 1);
    // The cursor move is more than 3 s before the newest line.
    assert_eq!(stream.recent_events().lines().count(), 5);

    use ElementKind::{Click, Marker, Typing, Url, Word};
    let kinds = [Word, Url, Click, Typing, Marker];
    let written = read(video, &take).unwrap().unwrap();
    assert_eq!(&written, stream.live());
    let quick = analyze(video, &t, &Speech::default(), Depth::Quick).unwrap();
    assert_eq!(lines(&written, &kinds), lines(&quick, &kinds));
}

#[test]
fn a_crash_keeps_the_streamed_words_and_analysis_writes_the_same_bytes_twice() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let take = Id::new("t01").unwrap();
    let t = transcript("We ship it. | On Fridays.");
    let mut stream = Stream::create(video, &take).unwrap();
    for word in t.words() {
        stream.word(word.clone());
    }
    stream.update().unwrap();
    drop(stream);
    let logged = read(video, &take).unwrap().unwrap();
    assert_eq!(logged.of_kind(ElementKind::Word).count(), 5);

    let media = video.join("media/t01");
    fs::create_dir_all(&media).unwrap();
    fs::write(
        media.join("events.jsonl"),
        "{\"t\":2000000,\"kind\":\"down\",\"x\":1,\"y\":2,\"button\":\"left\"}\n",
    )
    .unwrap();
    fs::write(video.join("script.md"), "# T\n\n## C\n\n- Beat\n").unwrap();
    let speech = Speech::default();
    let first = write(video, &analyze(video, &t, &speech, Depth::Full).unwrap()).unwrap();
    let bytes = fs::read(&first).unwrap();
    write(video, &analyze(video, &t, &speech, Depth::Full).unwrap()).unwrap();
    assert_eq!(fs::read(&first).unwrap(), bytes);
    let written = read(video, &take).unwrap().unwrap();
    assert_eq!(written.of_kind(ElementKind::Click).count(), 1);
    assert!(written.len() > logged.len());
}
