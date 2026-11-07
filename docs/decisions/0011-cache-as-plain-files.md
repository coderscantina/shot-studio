# 0011: The cache as plain files instead of SQLite

Status: Proposed

## Problem

The briefing puts peaks, thumbnails, transcripts and the render cache in SQLite under `.shot/cache`. Milestone 03 is the first to write cache data: transcripts and rendered segments.

## Constraints

- The cache is derived data. Deleting `.shot/cache` loses nothing.
- Rendered segments are large media files keyed by a content hash.
- Front ends and agents should be able to inspect the cache without extra tooling.

## Choice

Store each cache entry as a plain file named by its key: `.shot/cache/transcripts/<take>.json`, `.shot/cache/segments/<hash>.mp4`, and later `peaks/` and `thumbnails/` the same way. Add SQLite only when a query needs an index that a directory listing can't give.

## Rejected alternatives

- SQLite via sqlx from the start: a schema, migrations and an async runtime for data that is looked up by one key, while the segments still have to be files.

## Consequences

No sqlx dependency in milestone 03. Writes go to a temporary file and are renamed into place, so a crash never leaves a half-written entry. Evicting stale entries is a later task.

## Affected contracts

[Project files](../contracts/project-files.md) cache rules, `shot-analyze`, `shot-render`.
