//! A control channel for agents: with `SHOT_CONTROL=1` the app listens on the Unix socket
//! `$TMPDIR/shot-app-<pid>.sock` (user only, removed on quit). Each line is one JSON request,
//! answered by one JSON line, `{"ok":true,...}` or `{"ok":false,"error":"..."}`. A line that is
//! not a known request closes the connection.
//!
//! - `{"request":"windows"}`: `windows` with each window's `id`, `title` and `kind`.
//! - `{"request":"commands","window":ID}`: `commands` with each action's `name`, its `keys` in
//!   that window and whether it is `available` there now.
//! - `{"request":"dispatch","window":ID,"command":"edit::TogglePlay"}`: runs the command in that
//!   window only, refused when it is not available there.
//! - `{"request":"type","window":ID,"text":"..."}`: presses each character in that window, as
//!   typing into its focused input would.
//! - `{"request":"screenshot","window":ID,"path":"/abs/shot.png"}`: saves a PNG of that window.

use std::any::TypeId;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use gpui::{AnyView, AnyWindowHandle, App, AppContext, AsKeystroke, Keystroke, Window};
use gpui_component::Root;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Deserialize;
use serde_json::{Value, json};

/// How often the main thread picks up requests.
const POLL: Duration = Duration::from_millis(15);

#[derive(Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum Request {
    Windows,
    Commands { window: u64 },
    Dispatch { window: u64, command: String },
    Type { window: u64, text: String },
    Screenshot { window: u64, path: PathBuf },
}

type Reply = Result<Value, String>;

/// A request on its way to the main thread, with where its reply goes.
struct Job {
    request: Request,
    reply: Sender<Reply>,
}

/// Whether the environment asks for the control channel.
pub fn enabled() -> bool {
    std::env::var_os("SHOT_CONTROL").is_some_and(|value| value == "1")
}

/// Listens on the socket until the app quits. Prints the socket path, or why it could not open.
pub fn listen(cx: &mut App) {
    let path = std::env::temp_dir().join(format!("shot-app-{}.sock", std::process::id()));
    let listener = match bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("control channel {}: {error}", path.display());
            return;
        }
    };
    eprintln!("control channel on {}", path.display());
    cx.on_app_quit(move |_| {
        let _ = std::fs::remove_file(&path);
        async {}
    })
    .detach();

    let (jobs, inbox) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let jobs = jobs.clone();
            std::thread::spawn(move || serve(stream, jobs));
        }
    });
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            cx.update(|cx| drain(&inbox, cx));
        }
    })
    .detach();
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    // A file left by an earlier run with the same pid is stale.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Answers one connection line by line, until it closes or sends a line that is no request.
fn serve(stream: UnixStream, jobs: Sender<Job>) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(request) = line
            .map_err(drop)
            .and_then(|line| serde_json::from_str::<Request>(&line).map_err(drop))
        else {
            return;
        };
        let (reply, answer) = mpsc::channel();
        if jobs.send(Job { request, reply }).is_err() {
            return;
        }
        let Ok(reply) = answer.recv() else {
            return;
        };
        let reply = match reply {
            Ok(Value::Object(fields)) => {
                let mut reply = serde_json::Map::from_iter([("ok".into(), true.into())]);
                reply.extend(fields);
                Value::Object(reply)
            }
            Ok(_) => json!({ "ok": true }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        if writeln!(out, "{reply}").is_err() {
            return;
        }
    }
}

fn drain(inbox: &Receiver<Job>, cx: &mut App) {
    while let Ok(job) = inbox.try_recv() {
        handle(job, cx);
    }
}

fn handle(Job { request, reply }: Job, cx: &mut App) {
    let answer = match request {
        Request::Windows => Ok(windows(cx)),
        Request::Commands { window } => in_window(window, cx, commands),
        Request::Dispatch { window, command } => dispatch(window, &command, cx),
        Request::Type { window, text } => {
            in_window(window, cx, |window, cx| type_text(&text, window, cx))
        }
        Request::Screenshot { window, path } => {
            match in_window(window, cx, |window, _| window_number(window)) {
                Ok(Some(number)) => {
                    // screencapture takes a moment, so it runs off the main thread.
                    cx.background_spawn(async move {
                        let _ = reply.send(capture(number, &path));
                    })
                    .detach();
                    return;
                }
                Ok(None) => Err("the window has no native window".into()),
                Err(error) => Err(error),
            }
        }
    };
    let _ = reply.send(answer);
}

fn windows(cx: &mut App) -> Value {
    let windows: Vec<Value> = cx
        .windows()
        .into_iter()
        .filter_map(|handle| {
            handle
                .update(cx, |root, window, cx| {
                    json!({
                        "id": handle.window_id().as_u64(),
                        "title": window.window_title(),
                        "kind": kind(root, cx),
                    })
                })
                .ok()
        })
        .collect();
    json!({ "windows": windows })
}

/// Every action the keymap binds or the window can run, with the keys that reach it from the
/// window's focus.
fn commands(window: &mut Window, cx: &mut App) -> Value {
    let available: Vec<&'static str> = window
        .available_actions(cx)
        .iter()
        .map(|action| action.name())
        .collect();
    let mut commands: BTreeMap<&'static str, Vec<String>> =
        available.iter().map(|name| (*name, Vec::new())).collect();
    let contexts = window.context_stack();
    let os = cx.platform_info().os;
    for binding in cx.key_bindings().borrow().bindings() {
        let keys = commands.entry(binding.action().name()).or_default();
        if binding
            .predicate()
            .is_none_or(|predicate| predicate.eval(&contexts))
        {
            let stroke = binding
                .keystrokes()
                .iter()
                .map(|key| key.as_keystroke().unparse(os))
                .collect::<Vec<_>>()
                .join(" ");
            if !keys.contains(&stroke) {
                keys.push(stroke);
            }
        }
    }
    let commands: Vec<Value> = commands
        .into_iter()
        .map(|(name, keys)| {
            json!({ "name": name, "keys": keys, "available": available.contains(&name) })
        })
        .collect();
    json!({ "commands": commands })
}

fn dispatch(window: u64, command: &str, cx: &mut App) -> Reply {
    let action = cx
        .build_action(command, None)
        .map_err(|_| format!("no command {command}"))?;
    in_window(window, cx, |window, cx| {
        let available = window
            .available_actions(cx)
            .iter()
            .any(|other| other.partial_eq(action.as_ref()));
        if !available {
            return Err(format!("{command} is not available in this window now"));
        }
        window.dispatch_action(action, cx);
        Ok(Value::Null)
    })?
}

/// Presses each character, Enter for a newline and Tab for a tab.
fn type_text(text: &str, window: &mut Window, cx: &mut App) -> Value {
    for character in text.chars() {
        let keystroke = match character {
            '\n' => Keystroke::parse("enter").expect("enter parses"),
            '\t' => Keystroke::parse("tab").expect("tab parses"),
            _ => Keystroke {
                modifiers: Default::default(),
                key: character.to_string(),
                key_char: Some(character.to_string()),
            },
        };
        window.dispatch_keystroke(keystroke, cx);
    }
    Value::Null
}

/// Runs `f` in the window with this id, or says that there is none.
fn in_window<R>(
    id: u64,
    cx: &mut App,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> Result<R, String> {
    let handle: AnyWindowHandle = cx
        .windows()
        .into_iter()
        .find(|handle| handle.window_id().as_u64() == id)
        .ok_or_else(|| format!("no window {id}"))?;
    handle
        .update(cx, |_, window, cx| f(window, cx))
        .map_err(|error| error.to_string())
}

/// What a window shows, from the type of its root view.
fn kind(root: AnyView, cx: &App) -> &'static str {
    let view = match root.clone().downcast::<Root>() {
        Ok(root) => root.read(cx).view().entity_type(),
        Err(_) => root.entity_type(),
    };
    [
        (TypeId::of::<crate::studio::Studio>(), "studio"),
        (TypeId::of::<crate::board::BoardWindow>(), "board"),
        (TypeId::of::<crate::welcome::WelcomeView>(), "welcome"),
        (TypeId::of::<crate::library::LibraryWindow>(), "library"),
        (TypeId::of::<crate::brand::ThemeEditor>(), "theme"),
        (
            TypeId::of::<crate::brand::overlay::PlacedWindow>(),
            "overlay",
        ),
        (
            TypeId::of::<crate::layouts::builder::LayoutBuilder>(),
            "layout-builder",
        ),
        (TypeId::of::<crate::prompter::PrompterOutput>(), "prompter"),
        (TypeId::of::<crate::about::AboutView>(), "about"),
    ]
    .into_iter()
    .find(|(type_id, _)| *type_id == view)
    .map_or("other", |(_, kind)| kind)
}

/// The window's CoreGraphics window number, which `screencapture -l` takes.
fn window_number(window: &Window) -> Option<isize> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
    else {
        return None;
    };
    let view: *mut AnyObject = handle.ns_view.as_ptr().cast();
    // SAFETY: the view is gpui's live NSView, on the main thread; NSView.window and
    // NSWindow.windowNumber are plain getters.
    unsafe {
        let native: *mut AnyObject = msg_send![view, window];
        if native.is_null() {
            return None;
        }
        Some(msg_send![native, windowNumber])
    }
}

/// Saves the window as a PNG, without its shadow.
fn capture(number: isize, path: &Path) -> Reply {
    let status = Command::new("screencapture")
        .args(["-x", "-o", "-t", "png", "-l", &number.to_string()])
        .arg(path)
        .status()
        .map_err(|error| format!("screencapture: {error}"))?;
    if status.success() && path.exists() {
        Ok(json!({ "path": path }))
    } else {
        Err(format!("screencapture failed ({status})"))
    }
}
