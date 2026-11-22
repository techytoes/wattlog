pub mod cli;
pub mod telemetry;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use telemetry::processes::{force_kill_process_safe, quit_process_safe};
use telemetry::types::{
    ActionResult, DashboardSnapshot, KeepAwakeConfig, KeepAwakeState, WindowBounds,
};
use telemetry::TelemetryEngine;

const TRAY_ID: &str = "wattlog-tray";
const MAIN_WINDOW_LABEL: &str = "main";
const LAZY_DESTROY_DELAY_SECS: u64 = 60;

pub struct AppState {
    pub engine: Arc<TelemetryEngine>,
    pub destroy_gen: Arc<AtomicU64>,
    pub allow_exit: Arc<AtomicBool>,
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
fn hide_dashboard(app: AppHandle, state: State<'_, AppState>) {
    hide_and_schedule_destroy(&app, &state.engine, &state.destroy_gen);
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

fn open_or_show_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    // Cancel any pending 60s WebView destroy timer and wake on-demand `macmon` sampling.
    state.destroy_gen.fetch_add(1, Ordering::SeqCst);
    state.engine.set_dashboard_active(true);

    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let saved = state.engine.load_window_bounds();
    let width = saved.as_ref().map(|b| b.width as f64).unwrap_or(800.0);
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

    if let Some(ref b) = saved {
        builder = builder.position(b.x as f64, b.y as f64);
    } else {
        builder = builder.center();
    }

    if let Ok(win) = builder.build() {
        let _ = win.show();
        let _ = win.set_focus();

        let app_handle = app.clone();
        win.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let st = app_handle.state::<AppState>();
                hide_and_schedule_destroy(&app_handle, &st.engine, &st.destroy_gen);
            }
            _ => {}
        });
    }
}

fn hide_and_schedule_destroy(
    app: &AppHandle,
    engine: &Arc<TelemetryEngine>,
    destroy_gen: &Arc<AtomicU64>,
) {
    // Park `macmon::Sampler` immediately so hidden idle state has 0 macmon wakeups/sec.
    engine.set_dashboard_active(false);

    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) {
            let scale = win.scale_factor().unwrap_or(1.0);
            engine.save_window_bounds(&WindowBounds {
                x: (pos.x as f64 / scale).round() as i32,
                y: (pos.y as f64 / scale).round() as i32,
                width: (size.width as f64 / scale).round().max(680.0) as u32,
                height: (size.height as f64 / scale).round().max(440.0) as u32,
            });
        }
        let _ = win.hide();
    }

    let my_gen = destroy_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let gen_ref = Arc::clone(destroy_gen);
    let app_clone = app.clone();

    thread::spawn(move || {
        thread::sleep(Duration::from_secs(LAZY_DESTROY_DELAY_SECS));
        if gen_ref.load(Ordering::SeqCst) != my_gen {
            return;
        }
        if let Some(win) = app_clone.get_webview_window(MAIN_WINDOW_LABEL) {
            if !win.is_visible().unwrap_or(false) {
                let _ = win.destroy();
            }
        }
    });
}

fn toggle_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if win.is_visible().unwrap_or(false) {
            let st = app.state::<AppState>();
            hide_and_schedule_destroy(app, &st.engine, &st.destroy_gen);
            return;
        }
    }
    open_or_show_window(app);
}

pub fn run_gui(demo_mode: bool) {
    let engine = Arc::new(TelemetryEngine::new(demo_mode, true));
    let destroy_gen = Arc::new(AtomicU64::new(0));
    let allow_exit = Arc::new(AtomicBool::new(false));

    let state = AppState {
        engine: Arc::clone(&engine),
        destroy_gen: Arc::clone(&destroy_gen),
        allow_exit: Arc::clone(&allow_exit),
    };

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_or_show_window(app);
        }));

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
            hide_dashboard,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let open_item = MenuItem::with_id(app, "open", "Open Wattlog", true, None::<&str>)?;
            let awake_item =
                MenuItem::with_id(app, "toggle_awake", "Toggle Keep-Awake", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Wattlog", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &awake_item, &sep, &quit_item])?;

            let tray_bytes = include_bytes!("../icons/tray-template@2x.png");
            let tray_icon = Image::from_bytes(tray_bytes)?;

            let initial_power = engine.read_power_fast();
            let initial_title = format_tray_title(initial_power.system_watts, false);

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .icon_as_template(true)
                .title(initial_title)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => open_or_show_window(app),
                    "toggle_awake" => {
                        let st = app.state::<AppState>();
                        let power = st.engine.read_power_fast();
                        if let Ok(ka) = st.engine.keep_awake.toggle_quick(&power) {
                            update_tray_title(app, power.system_watts, ka.active);
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
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle());
                    }
                })
                .build(app)?;

            open_or_show_window(app.handle());

            // 24/7 lightweight Rust tray & SQLite logger loop (uses PSTR + AppleSmartBattery only, zero macmon overhead when hidden).
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
