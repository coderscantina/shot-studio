# 0040: The cut proposes itself, and the transcript is where it is reviewed

Status: Adopted by Mike on 2026-11-18. Amends [0028](0028-attempts-are-derived-ratings-are-written.md).

## Problem

A review on 2026-11-18 ran two spoken test videos through the app. "Assemble picks" without ratings took the longest attempt at a beat, which was the flubbed one both times. Three of three sentence restarts in one take were missed. A person reviewed the result in three places: the Cut step's queue by rule, the Takes grid by beat and the transcript in Edit. The product promises a cut that is ready after recording, with a person reducing and adapting it, and this was a cut a person had to assemble.

## Choice

- **The best scored attempt plays.** Every attempt at a beat gets a score from 0 to 100, computed like the attempt itself and never written: how few of its words are stumbles (fillers, restarts, misspoken words, pauses mid-sentence), how much of the beat's bullet and script it says, and whether it ends on a sentence end. The best of the attempts that say the beat whole plays, the newest wins a tie, so a retake of one sentence never stands for the beat. It runs after every take and again when the take's analysis lands, as one undo step. There is no Assemble step.
- **Sentences may come from another attempt.** The best attempt plays whole, unless another attempt says one of its sentences cleaner, with fewer fillers, restarted or misspoken words, or as clean and later, and leaves none of its content words out, its own stumbles not counted. That sentence then comes from the other attempt. A retake of half a beat therefore replaces that half.
- **A person's choice wins.** Switching a beat to another attempt writes a pick, and a pick stays when a later take scores better. A beat a person cut words in or answered a finding in is not picked again, unless they go back to its proposal themselves. Stars go: old ones are read and dropped on the next write.
- **Restarts are cut and shown.** A clear restart is cut at once. A doubtful one, and a doubled single word such as "very, very", asks first.
- **Without a script, a later take is appended.** Only sentences that say again what a sentence of an earlier take said compete with it, and a sentence it adds between two of them is kept as a new passage.
- **The transcript is the one review surface.** Text marks what is worth a look: restarts that were cut, unsure cuts, sentence splices between takes and beats whose attempts scored close. A count goes down as they are answered and never blocks Ship. Answers are overrides, as Keep and Cut are today, and for a beat a mark that its proposal was looked at, which is no pick. A beat with other attempts shows a marker, and one key steps through them in place.
- **The Cut step goes.** The nav is Record, Edit, Ship. Pacing and the rule switches move into a panel of Text. The Takes grid stays as a detail view opened from a beat, for comparing two attempts. The board keeps its Recorded column, whose cards open Edit.

## Rejected alternatives

- **Newest wins**: a worse retake replaces a good attempt until a person notices.
- **Claude ranks the attempts**: judgement the rules cannot make, but it needs a run per video and the cut would not be ready at stop. It can still propose a rewording as a diff.
- **Whole beats only**: fewer joins, but a partial retake, the common case, would lose to the full attempt it was recorded to fix.
- **A list that must reach zero before Ship**: turns a linter into a gate.

## Consequences

`Cut::assemble` and "Use this" stop being the normal path; composing is one core function the app, `shot ingest` and the MCP server share. The score depends on the retake detector, so its misses become wrong picks: the detector's fixtures are part of this work. Joins between takes sit in the gaps between words, half the gap and at most 300 ms on each side, so a join keeps a short pause.

## Affected contracts

[Project files](../contracts/project-files.md#takes), `shot-core`, `shot-analyze`, `shot-app`, `shot-cli`.
