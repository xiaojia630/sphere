//! Option+Space brings Sphere up over anything, and hides it again when it is in front. When something needs
//! you while Sphere is not in front, you get a macOS notification; in Sphere.app it has buttons (Allow, Always allow,
//! Deny, Reply, a question's options, Open: see `notifyact`). macOS only for now.
use crate::live::Live;
use crate::notifyact::{self, Act, Note};
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
    // first: the notification buttons don't need the hotkey (another app may hold Option+Space)
    native::authorize();
    native::setup();
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
        show_app();
    }
}

/// Brings Sphere forward: unhides it and activates it.
#[cfg(target_os = "macos")]
fn show_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.unhide(None);
    #[allow(deprecated)] // `activate()` alone doesn't come forward when started from a terminal
    app.activateIgnoringOtherApps(true);
}

#[cfg(not(target_os = "macos"))]
fn show_app() {}

/// The categories last registered with macOS.
type Registered = Vec<(String, Vec<Act>)>;

/// Posts a notification for each request that arrives while Sphere isn't the window in front. In Sphere.app the
/// notifications have buttons and come one per group of identical permission asks (a new identical ask replaces the
/// delivered one); one whose requests are all answered elsewhere is taken down. Elsewhere it is the plain
/// `osascript` one per request.
#[allow(clippy::too_many_arguments)]
pub fn notify(
    live: Res<Live>,
    win: Query<&Window, With<PrimaryWindow>>,
    args: Res<crate::Args>,
    mut seen: Local<Vec<String>>,
    mut first: Local<bool>,
    mut registered: Local<Registered>,
    mut posted: Local<Vec<String>>,
) {
    if args.offline() || !live.is_changed() {
        return;
    }
    let ids: Vec<String> = live.requests.iter().map(|r| r.info.id.clone()).collect();
    let rich = native_available();
    let notes = if rich { notifyact::plan(&live.requests, |t| live.title(t).to_string()) } else { Vec::new() };
    if rich {
        // the buttons of a question are registered before its notification is posted (and for the ones already waiting)
        let cats = notifyact::categories(&notes);
        if *registered != cats {
            register(&cats);
            *registered = cats;
        }
        let gone: Vec<String> = posted.iter().filter(|id| !notes.iter().any(|n| n.id == **id)).cloned().collect();
        if !gone.is_empty() {
            remove_delivered(&gone);
            posted.retain(|id| !gone.contains(id));
        }
    }
    // What was already waiting when the app started isn't news.
    if !*first {
        *first = true;
        *seen = ids;
        return;
    }
    let focused = win.single().map(|w| w.focused).unwrap_or(false);
    if rich {
        if !focused {
            for n in notes.iter().filter(|n| n.ids.iter().any(|id| !seen.contains(id))) {
                if post_note(n) && !posted.contains(&n.id) {
                    posted.push(n.id.clone());
                }
            }
        }
    } else {
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
    }
    *seen = ids;
}

/// A button pressed (or the notification clicked) in a notification: the action, the request ids the notification
/// carried, and the text typed into a Reply.
pub struct Reply {
    pub action: String,
    pub ids: Vec<String>,
    pub text: Option<String>,
}

/// Does what the notification buttons say: answers like the Agent moment card does, or brings Sphere forward on the
/// request. An answer to a request that was answered meanwhile is ignored by the service (see `service::command`).
pub fn from_notifications(
    live: Res<Live>,
    svc: Res<crate::service::Service>,
    mut m: ResMut<crate::moment::Moment>,
    mut sound: ResMut<crate::sound::Sound>,
    mut waiting_open: Local<Vec<(Reply, std::time::Instant)>>,
) {
    let mut todo: Vec<(Reply, std::time::Instant)> = std::mem::take(&mut *waiting_open);
    let now = std::time::Instant::now();
    todo.extend(take_replies().into_iter().map(|r| (r, now)));
    for (r, at) in todo {
        if notifyact::opens(&r.action) {
            show_app();
            // right after a launch from the notification the requests may not have arrived yet: try again for a while
            match notifyact::focus_task(&r.ids, &live.requests) {
                Some(task) => {
                    m.show_task(&live, &task, &mut sound);
                }
                None if live.requests.is_empty() && at.elapsed() < std::time::Duration::from_secs(15) => waiting_open.push((r, at)),
                None => {}
            }
            continue;
        }
        for a in notifyact::answers(&r.action, &r.ids, r.text.as_deref(), &live.requests) {
            m.mark_answered(&a.request);
            svc.send(crate::service::Cmd::Answer { request: a.request, answer: a.answer, text: a.text, always: a.always });
        }
    }
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

#[cfg(target_os = "macos")]
fn native_available() -> bool {
    native::bundled()
}
#[cfg(not(target_os = "macos"))]
fn native_available() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn register(cats: &[(String, Vec<Act>)]) {
    native::set_categories(cats);
}
#[cfg(not(target_os = "macos"))]
fn register(_cats: &[(String, Vec<Act>)]) {}

#[cfg(target_os = "macos")]
fn post_note(n: &Note) -> bool {
    native::post_note(n)
}
#[cfg(not(target_os = "macos"))]
fn post_note(_n: &Note) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn remove_delivered(ids: &[String]) {
    native::remove_delivered(ids);
}
#[cfg(not(target_os = "macos"))]
fn remove_delivered(_ids: &[String]) {}

#[cfg(target_os = "macos")]
fn take_replies() -> Vec<Reply> {
    native::take_replies()
}
#[cfg(not(target_os = "macos"))]
fn take_replies() -> Vec<Reply> {
    Vec::new()
}

/// UNUserNotificationCenter, available only inside an app bundle (it aborts in a bare binary, so check first).
#[cfg(target_os = "macos")]
mod native {
    use super::{Act, Note, Reply};
    use block2::DynBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSObject, NSObjectProtocol, NSSet, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
        UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNNotificationSound,
        UNTextInputNotificationAction, UNTextInputNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// What the delegate heard, until the Bevy system `from_notifications` takes it.
    static REPLIES: Mutex<Vec<Reply>> = Mutex::new(Vec::new());
    /// The userInfo key holding the request ids (one per line).
    const IDS_KEY: &str = "ids";

    define_class!(
        /// Hears the buttons. It does nothing but queue what it hears (it runs inside AppKit's event handling).
        #[unsafe(super(NSObject))]
        #[name = "SphereNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            // Sphere only posts while it is not in front, and a notification that arrives while it is stays unshown
            // (the default without a delegate), so say so.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(&self, _center: &UNUserNotificationCenter, _n: &UNNotification, done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
                done.call((UNNotificationPresentationOptions::empty(),));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &DynBlock<dyn Fn()>) {
                let action = response.actionIdentifier().to_string();
                let info = response.notification().request().content().userInfo();
                let ids = ids_of(&info);
                let text = response.downcast_ref::<UNTextInputNotificationResponse>().map(|r| r.userText().to_string());
                if let Ok(mut q) = REPLIES.lock() {
                    q.push(Reply { action, ids, text });
                }
                done.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    fn ids_of(info: &NSDictionary) -> Vec<String> {
        let key = NSString::from_str(IDS_KEY);
        let Some(v) = (unsafe { info.objectForKey_unchecked(&key) }) else { return Vec::new() };
        v.downcast_ref::<NSString>().map(|s| s.to_string().lines().filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default()
    }

    /// Sets the delegate (the center holds it weakly, so it lives for the whole run) and registers the fixed buttons.
    pub fn setup() {
        if !bundled() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate);
        set_categories(&super::notifyact::static_categories());
    }

    fn action(a: &Act) -> Retained<UNNotificationAction> {
        let (id, title) = (NSString::from_str(&a.id()), NSString::from_str(&a.title()));
        let options = match a {
            Act::Deny => UNNotificationActionOptions::Destructive,
            Act::Open => UNNotificationActionOptions::Foreground,
            _ => UNNotificationActionOptions::empty(),
        };
        match a.placeholder() {
            Some(p) => UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
                &id,
                &title,
                options,
                &NSString::from_str("Send"),
                &NSString::from_str(p),
            )
            .into_super(),
            None => UNNotificationAction::actionWithIdentifier_title_options(&id, &title, options),
        }
    }

    /// Replaces every registered category with `cats`.
    pub fn set_categories(cats: &[(String, Vec<Act>)]) {
        if !bundled() {
            return;
        }
        let list: Vec<Retained<UNNotificationCategory>> = cats
            .iter()
            .map(|(id, acts)| {
                let actions: Vec<Retained<UNNotificationAction>> = acts.iter().map(action).collect();
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(id),
                    &NSArray::from_retained_slice(&actions),
                    &NSArray::from_retained_slice(&[]),
                    UNNotificationCategoryOptions::empty(),
                )
            })
            .collect();
        UNUserNotificationCenter::currentNotificationCenter().setNotificationCategories(&NSSet::from_retained_slice(&list));
    }

    pub fn post_note(n: &Note) -> bool {
        if !bundled() {
            return false;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let c = UNMutableNotificationContent::new();
        c.setTitle(&NSString::from_str("Sphere"));
        c.setSubtitle(&NSString::from_str(&n.title));
        c.setBody(&NSString::from_str(&n.body));
        c.setSound(Some(&UNNotificationSound::defaultSound()));
        c.setCategoryIdentifier(&NSString::from_str(&n.category));
        let info = NSDictionary::from_retained_objects(&[&*NSString::from_str(IDS_KEY)], &[NSString::from_str(&n.ids.join("\n"))]);
        // SAFETY: a dictionary of strings is a dictionary of objects
        unsafe { c.setUserInfo(&*(Retained::as_ptr(&info) as *const NSDictionary<AnyObject, AnyObject>)) };
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&n.id), &c, None);
        center.addNotificationRequest_withCompletionHandler(&req, None);
        true
    }

    pub fn remove_delivered(ids: &[String]) {
        if !bundled() {
            return;
        }
        let ids: Vec<Retained<NSString>> = ids.iter().map(|i| NSString::from_str(i)).collect();
        UNUserNotificationCenter::currentNotificationCenter().removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
    }

    pub fn take_replies() -> Vec<Reply> {
        REPLIES.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }

    pub fn bundled() -> bool {
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
