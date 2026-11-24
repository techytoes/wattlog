pub mod cli;
pub mod telemetry;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, LogicalPosition, Manager, PhysicalPosition, Position, Rect, RunEvent, State,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartExt};

#[cfg(target_os = "macos")]
use tauri_nspanel::{
    tauri_panel, CollectionBehavior, ManagerExt as PanelManagerExt, PanelLevel, StyleMask,
    WebviewWindowExt as PanelWindowExt,
};

use telemetry::processes::{force_kill_process_safe, quit_process_safe};
use telemetry::types::{
    ActionResult, DashboardSnapshot, KeepAwakeConfig, KeepAwakeState, WindowBounds,
};
use telemetry::TelemetryEngine;

const TRAY_ID: &str = "wattlog-tray";
const MAIN_WINDOW_LABEL: &str = "main";
const LAZY_DESTROY_DELAY_SECS: u64 = 60;
const RESIGN_TOGGLE_DEBOUNCE_MS: i64 = 320;

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(WattlogPanel {
        config: {
            can_become_key_window: true,
            is_floating_panel: true
        }
    })

    panel_event!(WattlogPanelEventHandler {
        window_did_resign_key(notification: &NSNotification) -> ()
    })
}

pub struct AppState {
    pub engine: Arc<TelemetryEngine>,
    pub destroy_gen: Arc<AtomicU64>,
    pub allow_exit: Arc<AtomicBool>,
    pub pinned: Arc<AtomicBool>,
    pub last_resign_ms: Arc<AtomicI64>,
    pub last_tray_rect: Arc<Mutex<Option<Rect>>>,
    pub pin_menu_item: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
}

/// Runs off the main UI thread via `spawn_blocking` so heavy polls (`pmset` / `libproc`) never stall UI interactions.
#[tauri::command]
async fn get_dashboard_snapshot(
    state: State<'_, AppState>,
    include_sleep: Option<bool>,
    force_sleep_refresh: Option<bool>,
) -> Result<DashboardSnapshot, String> {
    let engine = Arc::clone(&state.engine);
    let inc = include_sleep.unwrap_or(false);
    let force = force_sleep_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || engine.snapshot(inc, force, false))
        .await
        .map_err(|e| format!("telemetry task failed: {e}"))
}

#[tauri::command]
fn toggle_keep_awake(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<KeepAwakeState, String> {
    let power = state.engine.read_power_fast();
    let ka = state.engine.keep_awake.toggle_quick(&power)?;
    update_tray_title(&app, power.system_watts, ka.active);
    Ok(ka)
}

#[tauri::command]
fn set_keep_awake(
    app: AppHandle,
    state: State<'_, AppState>,
    config: KeepAwakeConfig,
) -> Result<KeepAwakeState, String> {
    let power = state.engine.read_power_fast();
    let ka = state.engine.keep_awake.apply_config(config, &power)?;
    update_tray_title(&app, power.system_watts, ka.active);
    Ok(ka)
}

#[tauri::command]
fn quit_process(
    state: State<'_, AppState>,
    pid: i32,
    expected_start_secs: Option<u64>,
    app_name: Option<String>,
) -> ActionResult {
    if state.engine.is_demo() {
        return ActionResult {
            ok: true,
            message: format!(
                "[demo] Sent graceful Quit to {}",
                app_name.unwrap_or_else(|| format!("PID {pid}"))
            ),
        };
    }
    quit_process_safe(pid, expected_start_secs, app_name.as_deref())
}

#[tauri::command]
fn force_kill_process(
    state: State<'_, AppState>,
    pid: i32,
    expected_start_secs: Option<u64>,
    confirmed: bool,
) -> ActionResult {
    if !confirmed {
        return ActionResult {
            ok: false,
            message: "Force kill requires 2-keypress confirmation.".into(),
        };
    }
    if state.engine.is_demo() {
        return ActionResult {
            ok: true,
            message: format!("[demo] Force-killed PID {pid}"),
        };
    }
    force_kill_process_safe(pid, expected_start_secs, confirmed)
}

#[tauri::command]
fn get_window_pinned(state: State<'_, AppState>) -> bool {
    state.pinned.load(Ordering::SeqCst)
}

#[tauri::command]
fn set_window_pinned(state: State<'_, AppState>, pinned: bool) -> bool {
    state.pinned.store(pinned, Ordering::SeqCst);
    if let Ok(guard) = state.pin_menu_item.lock() {
        if let Some(ref item) = *guard {
            let _ = item.set_checked(pinned);
        }
    }
    pinned
}

#[tauri::command]
fn hide_dashboard(app: AppHandle) {
    hide_and_schedule_destroy(&app);
}

fn format_tray_title(watts: Option<f64>, awake: bool) -> String {
    let w = watts
        .map(|v| format!("{v:.1}W"))
        .unwrap_or_else(|| "--.-W".into());
    if awake {
        format!("{w} · ☕")
    } else {
        w
    }
}

fn update_tray_title(app: &AppHandle, watts: Option<f64>, awake: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let title = format_tray_title(watts, awake);
        let _ = tray.set_title(Some(title));
    }
}

/// Compute a clamped logical top-left position directly beneath the menu-bar tray icon.
fn compute_tray_anchored_position(
    app: &AppHandle,
    win: &WebviewWindow,
    tray_rect: Option<Rect>,
    win_width: f64,
    win_height: f64,
) -> Option<LogicalPosition<f64>> {
    let rect = tray_rect?;
    let scale = win.scale_factor().unwrap_or(2.0);

    let (rx, ry) = match rect.position {
        Position::Physical(p) => (p.x as f64 / scale, p.y as f64 / scale),
        Position::Logical(l) => (l.x, l.y),
    };
    let (rw, rh) = match rect.size {
        tauri::Size::Physical(s) => (s.width as f64 / scale, s.height as f64 / scale),
        tauri::Size::Logical(l) => (l.width, l.height),
    };

    let center_x = rx + rw / 2.0;
    let mut x = center_x - win_width / 2.0;
    let mut y = ry + rh + 6.0;

    // Clamp cleanly inside the monitor containing the tray icon.
    let monitors = app.available_monitors().unwrap_or_default();
    let target_mon = monitors
        .into_iter()
        .find(|m| {
            let ms = m.scale_factor();
            let mx = m.position().x as f64 / ms;
            let my = m.position().y as f64 / ms;
            let mw = m.size().width as f64 / ms;
            let mh = m.size().height as f64 / ms;
            center_x >= mx && center_x <= mx + mw && ry >= my && ry <= my + mh
        })
        .or_else(|| app.primary_monitor().ok().flatten());

    if let Some(mon) = target_mon {
        let ms = mon.scale_factor();
        let mx = mon.position().x as f64 / ms;
        let my = mon.position().y as f64 / ms;
        let mw = mon.size().width as f64 / ms;
        let mh = mon.size().height as f64 / ms;
        let margin = 12.0;
        let max_x = (mx + mw - win_width - margin).max(mx + margin);
        let max_y = (my + mh - win_height - margin).max(my + margin);
        x = x.clamp(mx + margin, max_x);
        y = y.clamp(my + 26.0, max_y);
    }

    Some(LogicalPosition::new(x.round(), y.round()))
}

fn position_window_at_anchor(
    app: &AppHandle,
    win: &WebviewWindow,
    anchor: Option<Rect>,
    fallback_bounds: Option<&WindowBounds>,
) {
    let scale = win.scale_factor().unwrap_or(2.0);
    let (w, h) = win
        .inner_size()
        .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
        .unwrap_or_else(|_| {
            (
                fallback_bounds.map(|b| b.width as f64).unwrap_or(780.0),
                fallback_bounds.map(|b| b.height as f64).unwrap_or(560.0),
            )
        });

    if let Some(pos) = compute_tray_anchored_position(app, win, anchor, w, h) {
        let _ = win.set_position(Position::Logical(pos));
    } else if let Some(b) = fallback_bounds {
        let _ = win.set_position(Position::Physical(PhysicalPosition::new(
            (b.x as f64 * scale).round() as i32,
            (b.y as f64 * scale).round() as i32,
        )));
    } else {
        let _ = win.center();
    }
}

fn is_dashboard_visible(app: &AppHandle) -> bool {
    #[cfg(target_os = "macos")]
    if let Ok(panel) = app.get_webview_panel(MAIN_WINDOW_LABEL) {
        return panel.is_visible();
    }
    app.get_webview_window(MAIN_WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn open_or_show_window(app: &AppHandle, anchor: Option<Rect>) {
    let state = app.state::<AppState>();
    // Cancel any pending 60s WebView destroy timer and wake on-demand `macmon` sampling.
    state.destroy_gen.fetch_add(1, Ordering::SeqCst);
    state.engine.set_dashboard_active(true);

    let effective_anchor = anchor.or_else(|| {
        state
            .last_tray_rect
            .lock()
            .ok()
            .and_then(|g| *g)
    });
    let saved = state.engine.load_window_bounds();

    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if !state.pinned.load(Ordering::SeqCst) || anchor.is_some() {
            position_window_at_anchor(app, &win, effective_anchor, saved.as_ref());
        }
        #[cfg(target_os = "macos")]
        if let Ok(panel) = app.get_webview_panel(MAIN_WINDOW_LABEL) {
            panel.show_and_make_key();
            return;
        }
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let width = saved.as_ref().map(|b| b.width as f64).unwrap_or(780.0);
    let height = saved.as_ref().map(|b| b.height as f64).unwrap_or(560.0);

    let mut builder = WebviewWindowBuilder::new(
        app,
        MAIN_WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("Wattlog")
    .inner_size(width, height)
    .min_inner_size(680.0, 440.0)
    .resizable(true)
    .visible(false)
    .decorations(true);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    if let Ok(win) = builder.build() {
        position_window_at_anchor(app, &win, effective_anchor, saved.as_ref());

        let close_handle = app.clone();
        win.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide_and_schedule_destroy(&close_handle);
            }
        });

        #[cfg(target_os = "macos")]
        {
            if let Ok(panel) = win.to_panel::<WattlogPanel>() {
                // Float cleanly over fullscreen Spaces without switching desktops or activating Dock.
                panel.set_level(PanelLevel::Status.value());
                panel.set_collection_behavior(
                    CollectionBehavior::new()
                        .can_join_all_spaces()
                        .full_screen_auxiliary()
                        .stationary()
                        .ignores_cycle()
                        .value(),
                );
                let _ = panel.add_style_mask(StyleMask::empty().nonactivating_panel().value());
                panel.set_hides_on_deactivate(false);
                panel.set_released_when_closed(false);

                let handler = WattlogPanelEventHandler::new();
                let resign_app = app.clone();
                handler.window_did_resign_key(move |_notification| {
                    let st = resign_app.state::<AppState>();
                    if st.pinned.load(Ordering::SeqCst) {
                        return;
                    }
                    st.last_resign_ms
                        .store(chrono::Utc::now().timestamp_millis(), Ordering::SeqCst);
                    hide_and_schedule_destroy(&resign_app);
                });
                panel.set_event_handler(Some(handler.as_ref()));

                panel.show_and_make_key();
                return;
            }
        }

        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn hide_and_schedule_destroy(app: &AppHandle) {
    let state = app.state::<AppState>();
    // Park `macmon::Sampler` immediately so hidden idle state has 0 macmon wakeups/sec.
    state.engine.set_dashboard_active(false);

    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
            let scale = win.scale_factor().unwrap_or(2.0);
            state.engine.save_window_bounds(&WindowBounds {
                x: (pos.x as f64 / scale).round() as i32,
                y: (pos.y as f64 / scale).round() as i32,
                width: (size.width as f64 / scale).round().max(680.0) as u32,
                height: (size.height as f64 / scale).round().max(440.0) as u32,
            });
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(panel) = app.get_webview_panel(MAIN_WINDOW_LABEL) {
            panel.hide();
        } else if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            let _ = win.hide();
        }
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = win.hide();
    }

    // Hybrid lazy-destroy: after 60s hidden, restore window ownership and destroy WebView to reclaim RAM (<20 MB idle).
    let my_gen = state.destroy_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let gen_ref = Arc::clone(&state.destroy_gen);
    let app_clone = app.clone();

    thread::spawn(move || {
        thread::sleep(Duration::from_secs(LAZY_DESTROY_DELAY_SECS));
        if gen_ref.load(Ordering::SeqCst) != my_gen {
            return;
        }
        let destroy_app = app_clone.clone();
        let _ = app_clone.run_on_main_thread(move || {
            if gen_ref.load(Ordering::SeqCst) != my_gen {
                return;
            }
            if is_dashboard_visible(&destroy_app) {
                return;
            }
            #[cfg(target_os = "macos")]
            {
                if let Some(panel) = destroy_app.remove_webview_panel(MAIN_WINDOW_LABEL) {
                    panel.set_event_handler(None);
                    if let Some(win) = panel.to_window() {
                        let _ = win.destroy();
                    }
                    return;
                }
            }
            if let Some(win) = destroy_app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = win.destroy();
            }
        });
    });
}

fn toggle_window_from_tray(app: &AppHandle, rect: Rect) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.last_tray_rect.lock() {
        *guard = Some(rect);
    }

    if is_dashboard_visible(app) {
        hide_and_schedule_destroy(app);
        return;
    }

    // Clicking the tray icon while the popover is open triggers `window_did_resign_key` right before `MouseButtonState::Up`.
    // Debounce briefly so that click cleanly closes the popover instead of immediately reopening it!
    let now_ms = chrono::Utc::now().timestamp_millis();
    let last_resign = state.last_resign_ms.load(Ordering::SeqCst);
    if now_ms - last_resign < RESIGN_TOGGLE_DEBOUNCE_MS {
        return;
    }

    open_or_show_window(app, Some(rect));
}

pub fn run_gui(demo_mode: bool) {
    let engine = Arc::new(TelemetryEngine::new(demo_mode, true));
    let destroy_gen = Arc::new(AtomicU64::new(0));
    let allow_exit = Arc::new(AtomicBool::new(false));
    let pinned = Arc::new(AtomicBool::new(false));
    let last_resign_ms = Arc::new(AtomicI64::new(0));
    let last_tray_rect = Arc::new(Mutex::new(None));

    let state = AppState {
        engine: Arc::clone(&engine),
        destroy_gen: Arc::clone(&destroy_gen),
        allow_exit: Arc::clone(&allow_exit),
        pinned: Arc::clone(&pinned),
        last_resign_ms: Arc::clone(&last_resign_ms),
        last_tray_rect: Arc::clone(&last_tray_rect),
        pin_menu_item: Mutex::new(None),
    };

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_or_show_window(app, None);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ));

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    let app = builder
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_dashboard_snapshot,
            toggle_keep_awake,
            set_keep_awake,
            quit_process,
            force_kill_process,
            get_window_pinned,
            set_window_pinned,
            hide_dashboard,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);

            let open_item = MenuItem::with_id(app, "open", "Open Wattlog", true, None::<&str>)?;
            let awake_item =
                MenuItem::with_id(app, "toggle_awake", "Toggle Keep-Awake", true, None::<&str>)?;
            let pin_item = CheckMenuItem::with_id(
                app,
                "toggle_pin",
                "Keep Window Pinned",
                true,
                false,
                None::<&str>,
            )?;
            let autostart_item = CheckMenuItem::with_id(
                app,
                "toggle_autostart",
                "Launch at Login",
                true,
                autostart_enabled,
                None::<&str>,
            )?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Wattlog", true, None::<&str>)?;

            if let Ok(mut slot) = app.state::<AppState>().pin_menu_item.lock() {
                *slot = Some(pin_item.clone());
            }

            let menu = Menu::with_items(
                app,
                &[
                    &open_item,
                    &awake_item,
                    &pin_item,
                    &autostart_item,
                    &sep,
                    &quit_item,
                ],
            )?;

            let tray_bytes = include_bytes!("../icons/tray-template@2x.png");
            let tray_icon = Image::from_bytes(tray_bytes)?;

            let initial_power = engine.read_power_fast();
            let initial_title = format_tray_title(initial_power.system_watts, false);

            let autostart_item_clone = autostart_item.clone();
            let pin_item_clone = pin_item.clone();

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .icon_as_template(true)
                .title(initial_title)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => open_or_show_window(app, None),
                    "toggle_awake" => {
                        let st = app.state::<AppState>();
                        let power = st.engine.read_power_fast();
                        if let Ok(ka) = st.engine.keep_awake.toggle_quick(&power) {
                            update_tray_title(app, power.system_watts, ka.active);
                        }
                    }
                    "toggle_pin" => {
                        let st = app.state::<AppState>();
                        let next = !st.pinned.load(Ordering::SeqCst);
                        st.pinned.store(next, Ordering::SeqCst);
                        let _ = pin_item_clone.set_checked(next);
                    }
                    "toggle_autostart" => {
                        let mgr = app.autolaunch();
                        let currently = mgr.is_enabled().unwrap_or(false);
                        let res = if currently {
                            mgr.disable()
                        } else {
                            mgr.enable()
                        };
                        if res.is_ok() {
                            let _ = autostart_item_clone.set_checked(!currently);
                        }
                    }
                    "quit" => {
                        let st = app.state::<AppState>();
                        st.allow_exit.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_window_from_tray(tray.app_handle(), rect);
                    }
                })
                .build(app)?;

            open_or_show_window(app.handle(), None);

            // 24/7 lightweight Rust tray & SQLite logger loop.
            let bg_handle = app.handle().clone();
            let bg_engine = Arc::clone(&engine);
            thread::Builder::new()
                .name("wattlog-tray-loop".into())
                .spawn(move || {
                    let mut last_title = String::new();
                    let mut tick_count: u64 = 0;

                    loop {
                        let record_sample = tick_count % 24 == 0;
                        let (power, ka) = bg_engine.tick_tray(record_sample);

                        let next_title = format_tray_title(power.system_watts, ka.active);
                        if next_title != last_title {
                            update_tray_title(&bg_handle, power.system_watts, ka.active);
                            last_title = next_title;
                        }

                        tick_count = tick_count.wrapping_add(1);
                        thread::sleep(Duration::from_millis(2500));
                    }
                })
                .ok();

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building wattlog");

    let exit_flag = Arc::clone(&allow_exit);
    app.run(move |_app_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            if !exit_flag.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
    });
}
