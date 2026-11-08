# Milestone 12 known gaps fixed

Each fix with the check that proves it. The camera warning and the toast need a live recording with a camera and a microphone, which this run did not have, so they are covered by their logic tests and by compiling into the Record screen.

## Camera black frames

`live.rs` reads the mean luma of each new camera preview frame (`shot_ship::media::Planes::mean`, every 8th pixel). Under 6 of 255 for 2 s sets `View::camera_black`, and the Record screen's Camera card shows "Black frames. Check the lens cover. The take still records." in the warn color. Nothing stops the recording. Check: `live::tests::the_camera_is_black_after_two_seconds_of_black_frames`. Not run against a real black camera.

## Proposal cache cleanup

`Board::delete_proposal` (accept and reject both end there) removes `videos/<slug>/.shot/cache/proposals/<name>` in the checked-out repo. The contract's layout tree names the folder. Check: `accepting_or_rejecting_removes_the_proposals_render_cache` in shot-board.

## URL reader

`github.com slash zedindustry slash zed.` already read whole when all its words were present; the clip's partial reads came from the words arriving one by one. The reader now also joins "forward slash", a lone "/", "period", "underscore" and a host split by a full stop ("github. com"). Check: `joins_whispers_half_spoken_forms_into_the_whole_url`. A real whisper run was not repeated.

## `style::shown` counts

Zooms and overlays of the lock go only into the block whose kept ranges they overlap, as review items already did. Check: `a_take_in_two_blocks_gets_each_block_only_the_zooms_that_play_in_it`; the frame equality test still passes.

## Inline retake toast

`shot_analyze::autocut::heard_retakes(words, confidence)` runs the repeat and restart parts of the retake rule over the last 80 settled words. The live thread calls it when a word settles and passes each new retake once to the prompter's `Live::lock().caught(Retake { heard, kept })`, at the format's auto-cut confidence. A retake shows once its clean version is as long as the attempt. Check: `a_retake_is_heard_once_its_clean_version_has_started`. Not seen on screen.

## `PcmReader::len`

`len()` (and `is_empty()`) give the track length in samples from the container, before any read. Check: `the_reader_knows_the_track_length_before_reading` writes a 0.5 s WAV and opens it. shot-render is untouched.

## Thumbnail location

The contract's layout tree names `.shot/export/main.png`. Doc only.

## Chapter note

Text between a `##` heading and its first beat is `ScriptChapter::note`, joined like a beat's script. It was an error before; files that parsed before parse identically. Text before the first chapter is still an error. Check: `text_between_a_chapter_heading_and_its_first_beat_is_the_chapter_note`.

## Checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test --workspace --no-fail-fast` green; the shot-app watchdog test failed once in the first run (the known flake) and passed on the rerun.
