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
fn a_crash_keeps_the_logged_words_and_analysis_writes_the_same_bytes_twice() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let take = Id::new("t01").unwrap();
    let t = transcript("We ship it. | On Fridays.");
    let log = Log::create(video, &take).unwrap();
    for word in t.words() {
        log.word(word).unwrap();
    }
    drop(log);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path(video, &take))
        .unwrap();
    file.write_all(br#"{"kind":"word","t":9000"#).unwrap();
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
