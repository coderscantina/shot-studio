# 0026: Library search on Openverse, downloads in the channel with a credit file

Status: Proposed on 2026-11-08 for milestone 13 items 4 and 8.

## Problem

Mike wants to search sound effects and memes from the app, hear or see them, download them into the channel and place them as a sound, a pop or a b-roll cutaway. The videos are monetized on YouTube, so every asset must allow commercial use, and its credit has to reach the video description. Until now sounds came from the bundled CC0 set or files put into `brand/sounds/` by hand, and nothing recorded where a file came from.

## Constraints

- Privacy ([0005](0005-privacy-text-only-to-claude.md)): nothing leaves the machine until a person searches, and the app says where the search goes.
- Only licenses that survive YouTube monetization and editing. A sound under a picture is an adaptation under Creative Commons, so "no derivatives" is out, and "share alike" would bind the whole video to CC BY-SA.
- The renderer draws PNG and JPEG stills; the mixer reads what AVFoundation decodes. No GIF decoder, no Ogg.
- The app has `ureq` for HTTP and adds no other network crate. No API keys or accounts in the first version.
- Assets a video uses must be on every machine that renders it, so they live in git like `brand/sounds/` does.

## Choice

- **Provider: Openverse** (`api.openverse.org/v1/audio/` and `/v1/images/`), anonymously. It indexes Freesound, Wikimedia, Flickr, Jamendo and more under CC licenses, needs no key and returns license, creator, source page and an attribution line for every result.
- **Filtered at the API and again on parse**: `license=cc0,pdm,by` and `extension=mp3,wav,flac` for sounds, `extension=jpg,png` for images, mature results dropped. GIFs are filtered out, not converted: a first frame would be a still that looks broken, and animated stills need a decoder and a renderer path of their own.
- **Search as you pause**: 450 ms after the last key, results are kept per query for the session. Anonymous clients get 20 searches a minute and 200 a day, 1000 thumbnails a day; running out is a plain notice, being offline an empty state.
- **Previews**: a sound plays from its file URL (Freesound's are the 128 kbit MP3 previews), fetched once into `~/Library/Caches/Shot Studio/library/`. An image shows Openverse's thumbnail, then the full image when selected.
- **Where downloads go** (channel repo, in git):
  - sounds into `brand/sounds/<name>.<ext>`, so `sfx <name>` finds them by the existing lookup, and the brand's `sounds` can name them;
  - images into `library/<name>.<ext>`, which the b-roll file list already offers as `../../library/`.
  - A video outside a channel keeps them in its own `sounds/` and `sources/`.
  - The name is the title as a sound name (`deep-whoosh-1`), numbered when taken. A result downloaded before is reused, by its Openverse id.
- **Credit file**: `<name>.credit.kdl` beside each download, parsed by `shot-core`: title, creator, license with version and URL, source page and provider, file URL, Openverse id, tags and the attribution line. See [Credits](../contracts/project-files.md#credits).
- **Pops draw images**: `pop` takes an optional `image` parameter, a still drawn `size` tall on the pop's springs, so a meme can pop in over the frame. A meme as a cutaway is `b-roll` with the file.
- **Credits for the description**: `shot_library::Folders::credits` lists the credits of every downloaded asset the cut uses (placed and automatic sounds, music, b-roll and pop files), and the Library window copies them as a block.

## Rejected alternatives

- **Freesound, Pixabay or Tenor directly**: each needs an API key or account, and Tenor's and Giphy's terms do not grant commercial reuse of the GIFs. Freesound is in Openverse anyway.
- **All commercial licenses (`license_type=commercial`)**: includes CC BY-SA and BY-ND, see the constraints.
- **Converting GIFs to their first frame or to a clip**: more machinery for a worse result, see above.
- **Downloads in a cache outside git**: a render on another machine would miss them.
- **Metadata in one catalog file per channel**: a merge conflict on every download from two videos; a sidecar moves and deletes with its file.

## Consequences

- The repo grows by the assets people download; images are limited to 25 MB and sounds to 40 MB each.
- 200 searches a day is enough for one person's editing. A registered Openverse application raises the limits and needs a client secret in Keychain, a follow-up if it runs short.
- Licenses can change at the source; the credit records what held on the download date's page.

## Affected contracts

[Project files](../contracts/project-files.md) (credit file, `library/`, the `pop` template's `image`), `shot-library`, `shot-render` (pop images), `shot-app` (Library window).
