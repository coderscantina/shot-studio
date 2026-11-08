//! The Library window, one per channel (or per video outside one), opened with ⌘L from a
//! studio window: the sounds, music, memes and images, overlays and effects a video can use,
//! with search across them, Openverse search for sounds and images, audition, download with
//! credit and placement into the Edit screen of the studio that opened it last.
//!
//! The list keys: arrows move, Space plays a sound or previews an image, Enter or D downloads
//! an Openverse result, P places (an image pops), B places an image as a cutaway, Esc or ⌘F
//! goes back to the search field. Tab switches Sounds and Memes and images between what is on
//! disk and Openverse. Nothing goes online until a person searches Openverse ([0026]).
//!
//! [0026]: ../../../../docs/decisions/0026-library-search-on-openverse.md

mod items;
mod view;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, ClipboardItem, Context, Entity, FocusHandle, Global,
    Pixels, ScrollHandle, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, actions, point, px, size,
};
use gpui_component::Root;
use gpui_component::input::{InputEvent, InputState};
use shot_audio::sound::Source;
use shot_core::{Credit, Id};
use shot_library::{Asset, Client, Folders, Hit, Media, Page};

use crate::edit::EditScreen;
use crate::edit::library::{Placement, Target};
use crate::sound::Audition;
use items::{Item, Section};

/// How long typing pauses before Openverse is asked.
const PAUSE: Duration = Duration::from_millis(450);
/// The size of a thumbnail or preview the window fetches at most.
const THUMB_LIMIT: u64 = 4 << 20;

actions!(
    library,
    [
        FocusSearch,
        SelectNext,
        SelectPrevious,
        SelectLeft,
        SelectRight,
        Listen,
        Primary,
        Download,
        Place,
        Cutaway,
        ToggleOnline,
        ShowAll,
        ShowSounds,
        ShowMusic,
        ShowImages,
        ShowOverlays,
        ShowEffects,
        CopyCredits,
        CloseLibrary,
    ]
);

pub(crate) const WINDOW: &str = "Library";
pub(crate) const LIST: &str = "LibraryList";

/// The open Library windows, by channel repo or loose video folder.
#[derive(Default)]
struct Libraries(Vec<(PathBuf, AnyWindowHandle, WeakEntity<LibraryWindow>)>);

impl Global for Libraries {}

/// Opens the Library of the video at `video`, or brings it to the front, placing into `edit`.
pub fn open(video: &Path, edit: WeakEntity<EditScreen>, cx: &mut App) -> Result<(), String> {
    let folders = Folders::of_video(video);
    let key = crate::studio::key(folders.channel.as_deref().unwrap_or(video));
    let open = cx.try_global::<Libraries>().and_then(|libraries| {
        libraries
            .0
            .iter()
            .find(|(k, ..)| *k == key)
            .and_then(|(_, window, view)| Some((*window, view.upgrade()?)))
    });
    if let Some((window, view)) = open {
        let shown = window.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_target(folders.clone(), edit.clone(), cx)
            });
            window.activate_window();
        });
        if shown.is_ok() {
            return Ok(());
        }
    }
    let title = match &folders.channel {
        Some(repo) => format!("Library of {}", crate::board::channel::name(repo)),
        None => "Library".to_owned(),
    };
    let bounds = Bounds::centered(None, size(px(1240.), px(780.)), cx);
    let mut opened = None;
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(980.), px(620.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| LibraryWindow::new(title, folders, edit, window, cx));
                opened = Some(view.downgrade());
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .map_err(|e| e.to_string())?;
    if let Some(view) = opened {
        let libraries = cx.default_global::<Libraries>();
        libraries
            .0
            .retain(|(k, _, v)| *k != key && v.upgrade().is_some());
        libraries.0.push((key, window.into(), view));
    }
    cx.activate(true);
    Ok(())
}

/// Something fetched in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fetch<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// A download running, with its bytes so far and its size once known (0 until then).
#[derive(Clone)]
struct Progress {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    /// Set to stop the download.
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Progress {
    /// The part done, from 0 to 1, once the size is known.
    fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f32 / total as f32)
    }
}

/// What the window says after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Note {
    text: String,
    error: bool,
}

/// Openverse's state for the query on show.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Online {
    /// No query yet, or typing has not paused.
    Idle,
    Searching,
    Ready,
    Failed {
        message: String,
        offline: bool,
    },
}

/// A search: the section's media and the query.
type SearchKey = (Media, String);

pub struct LibraryWindow {
    title: String,
    folders: Folders,
    /// The Edit screen placements go to.
    target: WeakEntity<EditScreen>,
    section: Section,
    /// Whether Sounds and Memes and images show Openverse instead of what is on disk.
    online: bool,
    input: Entity<InputState>,
    query: String,
    /// Everything on disk and built in, read again when the window comes back.
    local: Vec<Item>,
    /// Downloads by their Openverse id.
    downloaded: HashMap<String, Asset>,
    /// The selected item's key.
    selected: Option<String>,
    list_focus: FocusHandle,
    scroll: ScrollHandle,
    /// Where the image grid was last painted, for its columns.
    grid: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    client: Client,
    /// Result pages by section and query, kept for the window's life.
    pages: HashMap<SearchKey, Vec<Page>>,
    /// The pages being asked for.
    asking: std::collections::HashSet<(SearchKey, u32)>,
    /// Why the last search for a query failed, and whether for being offline.
    failed: HashMap<SearchKey, (String, bool)>,
    /// Counts query changes, so only the last pause in typing searches.
    generation: u64,
    thumbs: HashMap<String, Fetch<PathBuf>>,
    /// Openverse files fetched for listening or a large preview, by result id.
    files: HashMap<String, Fetch<PathBuf>>,
    downloads: HashMap<String, Progress>,
    audition: Option<Audition>,
    /// The item whose sound is loading to play.
    listening: Option<String>,
    /// The selected image shows large.
    large: bool,
    note: Option<Note>,
    credits: Vec<Credit>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryWindow {
    fn new(
        title: String,
        folders: Folders,
        target: WeakEntity<EditScreen>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search the library"));
        let typed = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::Change => {
                    this.query = input.read(cx).value().trim().to_owned();
                    this.query_changed(cx);
                }
                InputEvent::PressEnter { .. } => {
                    this.search_now(1, cx);
                    this.focus_list(window, cx);
                }
                _ => {}
            },
        );
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut this = Self {
            title,
            folders,
            target,
            section: Section::Sounds,
            online: false,
            input,
            query: String::new(),
            local: Vec::new(),
            downloaded: HashMap::new(),
            selected: None,
            list_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            grid: Default::default(),
            client: Client::new(),
            pages: HashMap::new(),
            asking: Default::default(),
            failed: HashMap::new(),
            generation: 0,
            thumbs: HashMap::new(),
            files: HashMap::new(),
            downloads: HashMap::new(),
            audition: None,
            listening: None,
            large: false,
            note: None,
            credits: Vec::new(),
            _subscriptions: vec![typed, activation],
        };
        this.reload(cx);
        this
    }

    /// Places into `target` from now on, the Edit screen of the video in `folders`.
    fn set_target(
        &mut self,
        folders: Folders,
        target: WeakEntity<EditScreen>,
        cx: &mut Context<Self>,
    ) {
        self.folders = folders;
        self.target = target;
        self.reload(cx);
    }

    /// Reads the folders and the target's credits again.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let brand = self
            .target
            .upgrade()
            .and_then(|edit| edit.read(cx).library_brand(cx));
        let assets = [
            self.folders.sounds(),
            self.folders.music(),
            self.folders.images(),
        ]
        .concat();
        self.downloaded = assets
            .iter()
            .filter_map(|a| Some((a.credit.as_ref()?.openverse.clone()?, a.clone())))
            .collect();
        self.local = assets.into_iter().map(Item::Asset).collect();
        let presets = self
            .folders
            .channel
            .as_deref()
            .map(|repo| crate::brand::channel_brand(repo).overlay_presets)
            .unwrap_or_default();
        self.local.extend(items::overlays(&presets, brand.as_ref()));
        self.local.extend(items::effects());
        self.credits = self
            .target
            .upgrade()
            .map(|edit| edit.read(cx).library_credits(cx))
            .unwrap_or_default();
        cx.notify();
    }

    fn target(&self, cx: &App) -> Option<Target> {
        Some(self.target.upgrade()?.read(cx).library_target(cx))
    }

    /// Whether the section on show lists Openverse results.
    fn showing_online(&self) -> Option<Media> {
        self.section.online().filter(|_| self.online)
    }

    /// The items on show, in order.
    fn visible(&self) -> Vec<Item> {
        if let Some(media) = self.showing_online() {
            return self
                .pages
                .get(&(media, self.query.clone()))
                .into_iter()
                .flatten()
                .flat_map(|page| page.hits.iter().cloned())
                .map(|hit| match self.downloaded.get(&hit.id) {
                    Some(asset) => Item::Asset(asset.clone()),
                    None => Item::Online(hit),
                })
                .collect();
        }
        let words = items::words(&self.query);
        let mut shown: Vec<Item> = self
            .local
            .iter()
            .filter(|item| self.section == Section::All || item.section() == self.section)
            .filter(|item| item.matches(&words))
            .cloned()
            .collect();
        if self.section == Section::All {
            // Grouped by section, in the sidebar's order.
            shown.sort_by_key(|item| Section::ALL.iter().position(|s| *s == item.section()));
        }
        shown
    }

    /// How many items of `section` match the query, on disk.
    fn count(&self, section: Section) -> usize {
        let words = items::words(&self.query);
        self.local
            .iter()
            .filter(|item| section == Section::All || item.section() == section)
            .filter(|item| item.matches(&words))
            .count()
    }

    fn selected_item(&self) -> Option<Item> {
        let visible = self.visible();
        let key = self.selected.as_ref();
        visible
            .iter()
            .find(|item| Some(&item.key()) == key)
            .or(visible.first())
            .cloned()
    }

    fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.selected = None;
        self.large = false;
        self.note = None;
        if self.showing_online().is_some() {
            self.search_now(1, cx);
        }
        cx.notify();
    }

    fn toggle_online(&mut self, _: &ToggleOnline, _: &mut Window, cx: &mut Context<Self>) {
        if self.section.online().is_none() {
            return;
        }
        self.online = !self.online;
        self.selected = None;
        self.large = false;
        if self.online {
            self.search_now(1, cx);
        }
        cx.notify();
    }

    fn set_online(&mut self, online: bool, cx: &mut Context<Self>) {
        if self.online != online {
            self.online = online;
            self.selected = None;
            if online {
                self.search_now(1, cx);
            }
            cx.notify();
        }
    }

    // Search.

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.selected = None;
        self.large = false;
        cx.notify();
        if self.showing_online().is_none() {
            return;
        }
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.search_now(1, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Openverse's state for the query on show.
    fn search(&self) -> Online {
        let Some(media) = self.showing_online() else {
            return Online::Idle;
        };
        let key = (media, self.query.clone());
        if self.asking.contains(&(key.clone(), 1)) {
            Online::Searching
        } else if self.pages.contains_key(&key) {
            Online::Ready
        } else if let Some((message, offline)) = self.failed.get(&key) {
            Online::Failed {
                message: message.clone(),
                offline: *offline,
            }
        } else {
            Online::Idle
        }
    }

    /// Whether more results of the query on show are on their way.
    fn loading_more(&self) -> bool {
        self.showing_online().is_some_and(|media| {
            self.asking
                .iter()
                .any(|((m, q), _)| *m == media && *q == self.query)
        })
    }

    /// Asks Openverse for `page` of the query on show, unless it is there or on its way.
    fn search_now(&mut self, page: u32, cx: &mut Context<Self>) {
        let Some(media) = self.showing_online() else {
            return;
        };
        let query = self.query.clone();
        let key = (media, query.clone());
        let have = self.pages.get(&key).map_or(0, Vec::len) as u32;
        if query.is_empty() || have >= page || !self.asking.insert((key.clone(), page)) {
            return;
        }
        self.failed.remove(&key);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = off_thread(cx, move || client.search(media, &query, page)).await;
            this.update(cx, |this, cx| {
                this.asking.remove(&(key.clone(), page));
                match result {
                    Ok(found) => {
                        let hits = found.hits.clone();
                        this.pages.entry(key).or_default().push(found);
                        for hit in hits {
                            this.fetch_thumb(&hit, cx);
                        }
                    }
                    Err(error) => {
                        let offline = matches!(error, shot_library::Error::Offline);
                        this.failed.insert(key, (error.to_string(), offline));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The next page, when the selection reaches the end of what is loaded.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(media) = self.showing_online() else {
            return;
        };
        let pages = self.pages.get(&(media, self.query.clone()));
        if let Some(last) = pages.and_then(|p| p.last())
            && last.has_more()
        {
            let next = last.page + 1;
            self.search_now(next, cx);
        }
    }

    fn fetch_thumb(&mut self, hit: &Hit, cx: &mut Context<Self>) {
        let Some(url) = hit
            .thumbnail
            .clone()
            .or_else(|| (hit.media == Media::Image).then(|| hit.file.clone()))
        else {
            return;
        };
        if self.thumbs.contains_key(&hit.id) {
            return;
        }
        self.thumbs.insert(hit.id.clone(), Fetch::Loading);
        let path = cache_dir().join(format!("{}.thumb", hit.id));
        let (client, id) = (self.client.clone(), hit.id.clone());
        cx.spawn(async move |this, cx| {
            let fetched = off_thread(cx, move || cached(&client, &url, &path, THUMB_LIMIT)).await;
            this.update(cx, |this, cx| {
                this.thumbs
                    .insert(id, fetched.map_or_else(Fetch::Failed, Fetch::Ready));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetches the file of `hit` into the cache, then calls `then` with its path.
    fn fetch_file(
        &mut self,
        hit: &Hit,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, PathBuf, &mut Context<Self>) + 'static,
    ) {
        match self.files.get(&hit.id) {
            Some(Fetch::Ready(path)) => return then(self, path.clone(), cx),
            Some(Fetch::Loading) => return,
            _ => {}
        }
        self.files.insert(hit.id.clone(), Fetch::Loading);
        let path = cache_dir().join(format!("{}.{}", hit.id, hit.extension));
        let (client, url, id, limit) = (
            self.client.clone(),
            hit.file.clone(),
            hit.id.clone(),
            hit.media.limit(),
        );
        cx.spawn(async move |this, cx| {
            let fetched = off_thread(cx, move || cached(&client, &url, &path, limit)).await;
            this.update(cx, |this, cx| {
                match fetched {
                    Ok(path) => {
                        this.files.insert(id, Fetch::Ready(path.clone()));
                        then(this, path, cx);
                    }
                    Err(error) => {
                        this.files.insert(id, Fetch::Failed(error.clone()));
                        this.say(error, true);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // Keys.

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        if self.large {
            self.large = false;
            cx.notify();
            return;
        }
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.selected = self.visible().first().map(Item::key);
        }
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    /// The number of items a row of the image grid holds.
    fn columns(&self) -> usize {
        if self.section != Section::Images {
            return 1;
        }
        let width = f32::from(self.grid.get().size.width);
        (((width + view::TILE_GAP) / (view::TILE_WIDTH + view::TILE_GAP)) as usize).max(1)
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|key| visible.iter().position(|i| &i.key() == key))
            .unwrap_or(0);
        let next = at.saturating_add_signed(by).min(visible.len() - 1);
        if next + self.columns() >= visible.len() {
            self.load_more(cx);
        }
        self.selected = Some(visible[next].key());
        self.scroll.scroll_to_item(view::child_index(
            &visible,
            next,
            self.section == Section::All,
        ));
        self.note = None;
        cx.notify();
    }

    fn listen(&mut self, _: &Listen, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            self.play(&item, cx);
        }
    }

    /// Plays a sound on its own, or stops it when it plays; shows an image large, or small
    /// again.
    fn play(&mut self, item: &Item, cx: &mut Context<Self>) {
        let key = item.key();
        self.selected = Some(key.clone());
        if item.still() {
            self.large = !self.large;
            if let Item::Online(hit) = item
                && self.large
            {
                self.fetch_file(hit, cx, |_, _, _| {});
            }
            cx.notify();
            return;
        }
        if !item.audible() {
            return;
        }
        let audition = self.audition.get_or_insert_with(Audition::spawn);
        let id = audition_id(&key);
        if audition.playing().as_ref() == Some(&id) || self.listening.as_ref() == Some(&key) {
            audition.stop();
            self.listening = None;
            cx.notify();
            return;
        }
        audition.stop();
        if let Some(edit) = self.target.upgrade() {
            edit.update(cx, |edit, cx| edit.pause_for_audition(cx));
        }
        match item {
            Item::Asset(asset) => {
                let source = match &asset.path {
                    Some(path) => Some(Source::File(path.clone())),
                    None => shot_audio::sound::bundled(&asset.name).map(Source::Bundled),
                };
                if let Some(source) = source {
                    audition.play(id, source);
                }
            }
            Item::Online(hit) => {
                self.listening = Some(key.clone());
                self.fetch_file(hit, cx, move |this, path, cx| {
                    if this.listening.as_ref() == Some(&key) {
                        this.listening = None;
                        if let Some(audition) = &this.audition {
                            audition.play(audition_id(&key), Source::File(path));
                        }
                        this.watch_audition(cx);
                    }
                });
            }
            _ => {}
        }
        self.watch_audition(cx);
        cx.notify();
    }

    /// Redraws while a sound plays, so its button turns back when it ends.
    fn watch_audition(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let Ok(playing) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.audition
                        .as_ref()
                        .is_some_and(|a| a.playing().is_some())
                }) else {
                    return;
                };
                if !playing {
                    return;
                }
            }
        })
        .detach();
    }

    fn primary(&mut self, _: &Primary, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_item() {
            Some(Item::Online(hit)) => self.download_then(&hit, None, cx),
            Some(item) => self.place(&item, false, cx),
            None => {}
        }
    }

    /// D: downloads the selected Openverse result, or stops its download.
    fn download_selected(&mut self, _: &Download, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_item() {
            Some(Item::Online(hit)) => self.download_then(&hit, None, cx),
            Some(Item::Asset(asset)) if asset.credit.is_some() => {
                self.say("Already downloaded".into(), false);
                cx.notify();
            }
            _ => {}
        }
    }

    fn place_selected(&mut self, _: &Place, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            self.place(&item, false, cx);
        }
    }

    fn cutaway_selected(&mut self, _: &Cutaway, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            self.place(&item, true, cx);
        }
    }

    /// Downloads `hit` into the channel with its credit, then places it when `place` says
    /// how: `Some(false)` as a sound or pop, `Some(true)` as a cutaway.
    fn download_then(&mut self, hit: &Hit, place: Option<bool>, cx: &mut Context<Self>) {
        if let Some(running) = self.downloads.get(&hit.id) {
            running.stop.store(true, Ordering::Relaxed);
            return;
        }
        let progress = Progress {
            done: Arc::default(),
            total: Arc::default(),
            stop: Arc::default(),
        };
        self.downloads.insert(hit.id.clone(), progress.clone());
        self.note = None;
        let cached = match self.files.get(&hit.id) {
            Some(Fetch::Ready(path)) => Some(path.clone()),
            _ => None,
        };
        let (client, folders, hit) = (self.client.clone(), self.folders.clone(), hit.clone());
        let id = hit.id.clone();
        let watch = progress.clone();
        cx.spawn(async move |this, cx| {
            let saved = off_thread(cx, move || {
                let bytes = match cached.and_then(|path| std::fs::read(path).ok()) {
                    Some(bytes) => bytes,
                    None => client
                        .fetch(&hit.file, hit.media.limit(), |done, total| {
                            progress.done.store(done, Ordering::Relaxed);
                            progress.total.store(total.unwrap_or(0), Ordering::Relaxed);
                            !progress.stop.load(Ordering::Relaxed)
                        })
                        .map_err(|e| e.to_string())?,
                };
                folders.save(&hit, &bytes)
            })
            .await;
            this.update(cx, |this, cx| {
                this.downloads.remove(&id);
                match saved {
                    Ok(saved) => {
                        this.reload(cx);
                        let shown = saved
                            .asset
                            .path
                            .as_deref()
                            .map_or_else(|| saved.asset.name.clone(), |p| this.folders.shown(p));
                        this.say(format!("Downloaded as {shown}"), false);
                        this.selected = Some(Item::Asset(saved.asset.clone()).key());
                        if let Some(cutaway) = place {
                            this.place(&Item::Asset(saved.asset), cutaway, cx);
                        }
                    }
                    Err(error) if error == shot_library::Error::Stopped.to_string() => {
                        this.say("Download stopped".into(), false)
                    }
                    Err(error) => this.say(format!("Not downloaded: {error}"), true),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        // Redraws the progress while the download runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let Ok(running) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.downloads
                        .values()
                        .any(|p| Arc::ptr_eq(&p.done, &watch.done))
                }) else {
                    return;
                };
                if !running {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Places `item` into the target Edit screen, an image as a cutaway when `cutaway`. An
    /// Openverse result is downloaded first.
    fn place(&mut self, item: &Item, cutaway: bool, cx: &mut Context<Self>) {
        if let Item::Online(hit) = item {
            if hit.media == Media::Image || !cutaway {
                self.download_then(hit, Some(cutaway), cx);
            }
            return;
        }
        if let Item::Template(spec) = item {
            return self.edit_overlay(spec.name, None, cx);
        }
        let folders = self.folders.clone();
        let relative = |asset: &Asset| asset.path.as_deref().and_then(|p| folders.in_video(p));
        let placement = if cutaway {
            item.cutaway(relative)
        } else {
            item.placement(relative)
        };
        let Some(placement) = placement else {
            if let Item::Effect(_) = item {
                self.say(
                    "Transitions and caption styles come from the format's rules. Set them in \
                     the Edit screen's style"
                        .into(),
                    false,
                );
                cx.notify();
            }
            return;
        };
        self.place_into_edit(placement, cx);
    }

    fn place_into_edit(&mut self, placement: Placement, cx: &mut Context<Self>) {
        let Some(edit) = self.target.upgrade() else {
            self.say(
                "Open the video's Edit screen and press ⌘L there to place into it".into(),
                true,
            );
            cx.notify();
            return;
        };
        let placed = edit.update(cx, |edit, cx| edit.place_from_library(placement, cx));
        match placed {
            Ok(done) => {
                let at = self.target(cx).map(|t| t.at).unwrap_or_default();
                self.say(format!("{done} at {at}. ⌘Z in the studio undoes it"), false);
                self.credits = edit.read(cx).library_credits(cx);
            }
            Err(error) => self.say(error, true),
        }
        cx.notify();
    }

    /// Opens the channel's overlay editor on `preset`, or on a new preset of `template`. A video
    /// outside a channel has no brand to keep presets in, so it places the overlay from Edit
    /// instead.
    fn edit_overlay(&mut self, template: &str, preset: Option<Id>, cx: &mut Context<Self>) {
        let Some(repo) = self.folders.channel.clone() else {
            self.say(
                format!(
                    "Overlay presets live in a channel's brand. Place a {template} from the Edit \
                     screen's Overlays pane, where its parameters are edited"
                ),
                false,
            );
            cx.notify();
            return;
        };
        let template = Id::new(template);
        if let Err(error) = crate::brand::open_overlay_preset(&repo, preset, template, cx) {
            self.say(error, true);
            cx.notify();
        }
    }

    fn copy_credits(&mut self, _: &CopyCredits, _: &mut Window, cx: &mut Context<Self>) {
        let text = shot_library::catalog::description(&self.credits);
        if text.is_empty() {
            self.say("This cut uses no downloaded assets yet".into(), false);
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.say(
                format!(
                    "Copied {} for the video description",
                    plural(self.credits.len(), "credit")
                ),
                false,
            );
        }
        cx.notify();
    }

    fn say(&mut self, text: String, error: bool) {
        self.note = Some(Note { text, error });
    }
}

impl gpui::Focusable for LibraryWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.list_focus.clone()
    }
}

/// Runs blocking network or disk work on a thread of its own, so a slow server never holds up
/// gpui's background executor, and waits for it without blocking the UI.
async fn off_thread<T: Send + 'static>(
    cx: &mut gpui::AsyncApp,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let slot = Arc::new(std::sync::Mutex::new(None));
    let filled = slot.clone();
    std::thread::spawn(move || {
        let value = work();
        *filled.lock().expect("an off-thread slot") = Some(value);
    });
    loop {
        if let Some(value) = slot.lock().expect("an off-thread slot").take() {
            return value;
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
}

/// Where previews and thumbnails are kept: `~/Library/Caches/Shot Studio/library`.
fn cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Caches/Shot Studio/library")
}

/// The file at `url` in the cache at `path`, fetched once.
fn cached(client: &Client, url: &str, path: &Path, limit: u64) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_owned());
    }
    let bytes = client
        .fetch(url, limit, |_, _| true)
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(cache_dir()).map_err(|e| e.to_string())?;
    shot_fs::write(path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_owned())
}

/// The name the audition knows an item's sound by.
fn audition_id(key: &str) -> Id {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    Id::new(format!("a{:x}", hasher.finish())).expect("a letter and hex digits")
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}
