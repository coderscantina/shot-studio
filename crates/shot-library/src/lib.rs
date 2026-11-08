//! The Library: the sounds, music and images a video can use, on disk and online.
//!
//! - [`catalog`] lists what is on disk (the bundled set, the channel's and the video's own
//!   files, downloads with their credit), saves downloads with their credit beside them and
//!   lists the credits a cut uses for the video description.
//! - [`openverse`] searches Openverse for sounds and images under licenses that allow
//!   commercial use, and fetches previews and files. Nothing goes online until a person
//!   searches ([0026]).
//!
//! [0026]: ../../../docs/decisions/0026-library-search-on-openverse.md

pub mod catalog;
pub mod openverse;

pub use catalog::{Asset, Folders, Origin, Saved, Section};
pub use openverse::{Client, Error, Hit, Media, Page};
