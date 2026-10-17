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
    native::authorize();
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state() == HotKeyState::Pressed {
            toggle();
        }
    }));
    app.insert_non_send(Hotkey(manager));
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &mut App) {}

/// Sphere stays resident: closing the window (the close button) hides it instead of quitting, and Cmd+Q quits.
/// While it is hidden or fully covered nothing draws: every camera is off (their view textures are freed after a
/// few frames) and the loop wakes only for events and twice a second for live updates. It shows again in one
/// frame. Visible, it runs at full rate even when another app has focus (Bevy's default caps that at 60 Hz).
pub fn residency(
    mut occluded: MessageReader<bevy::window::WindowOccluded>,
    mut closes: MessageReader<bevy::window::WindowCloseRequested>,
    mut cams: Query<&mut Camera>,
    mut ws: ResMut<bevy::winit::WinitSettings>,
    mut hidden: Local<bool>,
    mut exit: MessageWriter<AppExit>,
    args: Res<crate::Args>,
) {
    use bevy::winit::UpdateMode;
    if closes.read().count() > 0 {
        if cfg!(target_os = "macos") && args.bench.is_none() {
            hide();
        } else {
            exit.write(AppExit::Success);
        }
    }
    let mut now = *hidden;
    for e in occluded.read() {
        // a bench measures drawing, covered or not
        now = e.occluded && args.bench.is_none();
    }
    if now != *hidden {
        *hidden = now;
        if now {
            let idle = UpdateMode::reactive_low_power(std::time::Duration::from_millis(500));
            *ws = bevy::winit::WinitSettings { focused_mode: idle, unfocused_mode: idle };
        } else {
            // everything draws once (panels that changed while hidden catch up); the panel gate trims it next frame
            for mut c in &mut cams {
                c.is_active = true;
            }
            *ws = visible_settings();
        }
    }
    if *hidden {
        for mut c in &mut cams {
            if c.is_active {
                c.is_active = false;
            }
        }
    }
}

pub fn visible_settings() -> bevy::winit::WinitSettings {
    bevy::winit::WinitSettings { focused_mode: bevy::winit::UpdateMode::Continuous, unfocused_mode: bevy::winit::UpdateMode::Continuous }
}

#[cfg(target_os = "macos")]
fn hide() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
    }
}

#[cfg(not(target_os = "macos"))]
fn hide() {}

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

/// Native notifications when Sphere runs as Sphere.app (they show under its name); `osascript` otherwise (with the
/// text as arguments, never spliced into the script; those show under Script Editor's name).
fn post(title: &str, body: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let body: String = body.chars().take(180).collect();
    #[cfg(target_os = "macos")]
    if native::post(title, &body) {
        return;
    }
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

/// UNUserNotificationCenter, available only inside an app bundle (it aborts in a bare binary, so check first).
#[cfg(target_os = "macos")]
mod native {
    use objc2_foundation::{NSBundle, NSString};
    use objc2_user_notifications::{UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    pub fn authorize() {
        if !bundled() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let done = block2::RcBlock::new(|_ok: objc2::runtime::Bool, _err: *mut objc2_foundation::NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
    }

    pub fn post(title: &str, body: &str) -> bool {
        if !bundled() {
            return false;
        }
        static N: AtomicU64 = AtomicU64::new(0);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let c = UNMutableNotificationContent::new();
        c.setTitle(&NSString::from_str("Sphere"));
        c.setSubtitle(&NSString::from_str(title));
        c.setBody(&NSString::from_str(body));
        c.setSound(Some(&UNNotificationSound::defaultSound()));
        let id = NSString::from_str(&format!("sphere-{}", N.fetch_add(1, Ordering::Relaxed)));
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &c, None);
        center.addNotificationRequest_withCompletionHandler(&req, None);
        true
    }
}
