# 0009: Ship to YouTube, export the rest as files

Status: Adopted by Mike on 2026-11-07

## Problem

Which platforms `shot ship` publishes to in the first release.

## Constraints

- The main video and Shorts go to YouTube.
- X's media API needs a paid developer tier.

## Choice

`shot ship` uploads the main video and Shorts to YouTube through the Data API, with chapters, description and thumbnail, and tags the release in git. The OAuth token lives in Keychain. The X clip and any other cut are exported as files.

## Rejected alternatives

- YouTube and X uploads: a paid and unstable API for one clip a week.
- Files only: loses the release-as-ship step.

## Consequences

Milestone 08 needs a Google Cloud project with an OAuth desktop client before the upload can be verified.

## Affected contracts

`shot-ship`, `shot-cli`.
