//! Option+Space brings Sphere up over anything, and hides it again when it is in front. When something needs
//! you while Sphere is not in front, you get a macOS notification. macOS only for now.
use crate::live::Live;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// Keeps the hotkey registered for the app's lifetime (the manager unregisters on drop).
#[cfg(target_os = "macos")]
pub struct Hotkey(#[allow(dead_code)] global_hotkey::GlobalHotKeyManager);

/// Registers Option+Space. The handler runs on the main thread from the app's event loop, and toggles the
/// app with AppKit directly, so it works while the window is hidden and Bevy isn't drawing.
#[cfg(target_os = "macos")]
pub fn install(app: &mut App) {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("no global hotkey: {e}");
            return;
        }
    };
    if let Err(e) = manager.register(HotKey::new(Some(Modifiers::ALT), Code::Space)) {
        eprintln!("could not register Option+Space: {e}");
        return;
    }
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state() == HotKeyState::Pressed {
            toggle();
        }
    }));
    app.insert_non_send(Hotkey(manager));
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &mut App) {}

#[cfg(target_os = "macos")]
fn toggle() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    if app.isActive() {
        app.hide(None);
    } else {
        app.unhide(None);
        #[allow(deprecated)] // `activate()` alone doesn't come forward when started from a terminal
        app.activateIgnoringOtherApps(true);
    }
}

/// Posts a notification for each request that arrives while Sphere isn't the window in front.
pub fn notify(live: Res<Live>, win: Query<&Window, With<PrimaryWindow>>, args: Res<crate::Args>, mut seen: Local<Vec<String>>, mut first: Local<bool>) {
    if args.offline() || !live.is_changed() {
        return;
    }
    let ids: Vec<String> = live.requests.iter().map(|r| r.info.id.clone()).collect();
    // What was already waiting when the app started isn't news.
    if !*first {
        *first = true;
        *seen = ids;
        return;
    }
    let focused = win.single().map(|w| w.focused).unwrap_or(false);
    for r in &live.requests {
        if seen.contains(&r.info.id) {
            continue;
        }
        if !focused {
            let title = live.title(&r.info.task).to_string();
            let what = if r.info.summary.is_empty() { r.info.tool.clone() } else { r.info.summary.clone() };
            post(&format!("Needs you · {title}"), &what);
        }
    }
    *seen = ids;
}

/// `osascript` with the text as arguments (never spliced into the script). Shown under Script Editor's name
/// until Sphere ships as an app bundle with its own notification rights.
fn post(title: &str, body: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let body: String = body.chars().take(180).collect();
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title \"Sphere\" subtitle (item 1 of argv) sound name \"Glass\"",
            "-e",
            "end run",
            "--",
            title,
            &body,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
