//! Klirr desktop app: the web UI (`web/`, built in desktop mode) in the system webview, and
//! the amp on the native audio host (`klirr-native`), which gets the latency the browser
//! cannot: ASIO, WASAPI exclusive, Core Audio, JACK.
//!
//! The webview only shows the UI. It calls the commands below; the audio runs on the
//! [`audio`] controller's thread and the streams' own threads, never in the webview.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod log;

use std::fs;
use std::sync::Mutex;
use std::time::Duration;

use klirr_dsp::Param;
use klirr_native::devices::{self, DeviceInfo};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{Manager, RunEvent, State};

use audio::{Audio, Event, MeasureReply, StartRequest, Started};
use config::{Config, Status, Store};

/// An audio host, for the driver menu.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostEntry {
    id: String,
    name: String,
    is_default: bool,
    /// What the Hardware Setup button opens; None where there is nothing to open.
    hardware_setup: Option<&'static str>,
}

/// A device, for the device menus.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceEntry {
    name: String,
    channels: u16,
    is_default: bool,
}

#[derive(Serialize)]
struct DeviceLists {
    inputs: Vec<DeviceEntry>,
    outputs: Vec<DeviceEntry>,
}

fn entries(list: Vec<DeviceInfo>) -> Vec<DeviceEntry> {
    list.into_iter()
        .map(|d| DeviceEntry {
            name: d.name,
            channels: d.channels,
            is_default: d.is_default,
        })
        .collect()
}

#[tauri::command]
fn hosts() -> Vec<HostEntry> {
    devices::hosts()
        .into_iter()
        .map(|h| HostEntry {
            id: h.id,
            name: h.name,
            is_default: h.is_default,
            hardware_setup: h.hardware_setup,
        })
        .collect()
}

/// Lists a host's devices (empty `host`: the default host). Can take a moment (ASIO loads its
/// drivers), so it runs off the main thread.
#[tauri::command(async)]
fn devices(host: String) -> Result<DeviceLists, String> {
    let list = devices::list((!host.is_empty()).then_some(host.as_str())).map_err(|e| {
        log::error(format!("listing the devices of {host:?}: {e}"));
        e.to_string()
    })?;
    Ok(DeviceLists {
        inputs: entries(list.inputs),
        outputs: entries(list.outputs),
    })
}

/// The buffer sizes and sample rates a host's devices allow.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferChoice {
    sample_rate: u32,
    rates: Vec<u32>,
    default: Option<u32>,
    sizes: Vec<u32>,
    set_by: Option<&'static str>,
}

/// Buffer sizes and sample rates for a host and devices (empty strings: the defaults), the
/// sizes counted at `rate` where Klirr chooses the rate. Like [`devices()`], it can load
/// drivers, so it runs off the main thread.
#[tauri::command(async)]
fn buffer_sizes(
    host: String,
    input: String,
    output: String,
    rate: Option<u32>,
) -> Result<BufferChoice, String> {
    let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let (host, input, output) = (some(&host), some(&input), some(&output));
    let c = devices::buffer_choice(host.as_deref(), input.as_deref(), output.as_deref(), rate)
        .map_err(|e| {
            log::error(format!("asking {host:?} for its buffer sizes: {e}"));
            e.to_string()
        })?;
    Ok(BufferChoice {
        sample_rate: c.sample_rate,
        rates: c.rates,
        default: c.default,
        sizes: c.sizes,
        set_by: c.set_by,
    })
}

/// The settings file, and what it holds. None if the config folder could not be found.
struct Settings(Mutex<Option<(Store, Loaded)>>);

/// What [`load_config`] returns.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Loaded {
    config: Config,
    /// How the file was found when the app started.
    status: Status,
    /// Where the file is, to show the user.
    path: String,
    /// Where an unreadable file was moved to (with [`Status::Broken`]).
    broken_path: Option<String>,
}

const NO_SETTINGS: &str = "the settings folder could not be found";

/// The settings: as the app found them at start, with what was saved since.
#[tauri::command]
fn load_config(settings: State<'_, Settings>) -> Result<Loaded, String> {
    let guard = settings.0.lock().map_err(|_| NO_SETTINGS)?;
    guard
        .as_ref()
        .map(|(_, loaded)| loaded.clone())
        .ok_or_else(|| NO_SETTINGS.to_owned())
}

#[tauri::command]
fn save_config(settings: State<'_, Settings>, config: Config) -> Result<(), String> {
    let mut guard = settings.0.lock().map_err(|_| NO_SETTINGS)?;
    let (store, loaded) = guard.as_mut().ok_or(NO_SETTINGS)?;
    store.save(&config).map_err(|e| {
        log::error(format!("saving the settings: {e}"));
        format!(
            "could not save the settings to {}: {e}",
            store.path().display()
        )
    })?;
    loaded.config = config;
    Ok(())
}

/// Opens the settings file in the app's config folder.
fn open_settings(app: &tauri::App) -> Settings {
    let Ok(dir) = app.path().app_config_dir() else {
        return Settings(Mutex::new(None));
    };
    if config::adopt_legacy(&dir, config::LEGACY_IDENTIFIER) {
        log::info(format!(
            "settings copied from the {} folder",
            config::LEGACY_IDENTIFIER
        ));
    }
    let (store, config, status) = Store::open(&dir);
    log::info(format!("settings file: {status:?}"));
    let loaded = Loaded {
        config,
        status,
        path: store.path().display().to_string(),
        broken_path: (status == Status::Broken).then(|| store.broken_path().display().to_string()),
    };
    Settings(Mutex::new(Some((store, loaded))))
}

#[tauri::command(async)]
fn start(
    audio: State<'_, Audio>,
    request: StartRequest,
    events: Channel<Event>,
) -> Result<Started, String> {
    audio.start(request, events)
}

#[tauri::command(async)]
fn stop(audio: State<'_, Audio>) -> Result<(), String> {
    audio.stop()
}

/// Parameter changes as `[index, value]` pairs; unknown indices are ignored.
#[tauri::command]
fn set_params(audio: State<'_, Audio>, changes: Vec<(u32, f32)>) -> Result<(), String> {
    audio.set_params(
        changes
            .into_iter()
            .filter_map(|(i, v)| Param::from_index(i).map(|p| (p, v)))
            .collect(),
    )
}

/// A custom IR: the raw body holds little-endian f32 samples, the `x-sample-rate` header
/// their rate. Decoding the file is left to the webview, as in the browser.
#[tauri::command]
fn load_ir(audio: State<'_, Audio>, request: Request<'_>) -> Result<(), String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the samples as raw bytes".to_owned());
    };
    let rate: f32 = request
        .headers()
        .get("x-sample-rate")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or("missing x-sample-rate header")?;
    let (words, _) = bytes.as_chunks::<4>();
    let samples = words.iter().map(|&b| f32::from_le_bytes(b)).collect();
    audio.load_ir(samples, rate)
}

#[tauri::command(async)]
fn measure(audio: State<'_, Audio>) -> Result<MeasureReply, String> {
    audio.measure()
}

#[tauri::command]
fn test_tone(audio: State<'_, Audio>, on: bool) -> Result<(), String> {
    audio.test_tone(on)
}

/// Opens the hardware setup for a host and output device (empty strings: the defaults). An
/// ASIO driver's panel can keep it waiting until the user closes it, so it runs off the main
/// thread.
#[tauri::command(async)]
fn hardware_setup(audio: State<'_, Audio>, host: String, output: String) -> Result<(), String> {
    let some = |s: String| (!s.is_empty()).then_some(s);
    audio.hardware_setup(some(host), some(output))
}

/// What the Help panel's report needs from the native side.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    /// The app's version, as built.
    version: &'static str,
    /// Whether this build has ASIO (and so is GPLv3, see the README).
    asio: bool,
    /// The operating system and its version, e.g. "Windows 11 Pro 10.0.26300 (64-bit)".
    os: String,
    arch: &'static str,
    /// The system webview's version (WebView2, WebKitGTK, WKWebView).
    webview: String,
    /// The log's recent lines, oldest first.
    log: Vec<String>,
    /// The last lines of the previous run, if it did not end cleanly (a crash).
    previous_log: Option<Vec<String>>,
    /// Where the log file is, with the home folder as `~`.
    log_path: Option<String>,
}

/// "Windows 11 Pro 10.0.26300": the edition where the system names one, and the version.
fn describe_os(os: &os_info::Info) -> String {
    let name = os
        .edition()
        .map_or_else(|| os.os_type().to_string(), str::to_owned);
    format!("{name} {}", os.version())
}

#[tauri::command]
fn diagnostics() -> Diagnostics {
    let os = os_info::get();
    let snapshot = log::snapshot();
    Diagnostics {
        version: env!("CARGO_PKG_VERSION"),
        asio: cfg!(feature = "asio"),
        os: format!("{} ({})", describe_os(&os), os.bitness()),
        arch: std::env::consts::ARCH,
        webview: tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})")),
        log: snapshot.lines,
        previous_log: snapshot.previous,
        log_path: snapshot.path,
    }
}

/// A line for the log from the UI: what it told the user.
#[tauri::command]
fn log_ui(message: String) {
    log::ui(message.chars().take(500).collect::<String>());
}

/// Saves a bug report as `name` in the Downloads folder and shows it there. Returns where it
/// is. The name is the UI's (it is in the email too): `klirr-report-…` and `.txt` only.
#[tauri::command(async)]
fn save_report(app: tauri::AppHandle, name: String, text: String) -> Result<String, String> {
    if !report_name_allowed(&name) {
        return Err(format!("not a report file name: {name}"));
    }
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().document_dir())
        .map_err(|e| format!("no folder to save the report in: {e}"))?;
    let path = dir.join(&name);
    fs::write(&path, text).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    log::info(format!("bug report saved as {name}"));
    // Showing it is a convenience: the path is shown either way.
    let _ = tauri_plugin_opener::reveal_item_in_dir(&path);
    Ok(path.display().to_string())
}

fn report_name_allowed(name: &str) -> bool {
    name.starts_with("klirr-report-")
        && name.ends_with(".txt")
        && name.len() < 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !name.contains("..")
}

/// Where the Help panel's links may lead: the support address, the website, the source.
/// An entry ending in `/` or `?` allows what follows it; the entry without it is allowed too.
const LINKS: &[&str] = &[
    "mailto:support@bechtstudio.com?",
    "https://bechtstudio.com/",
    "https://klirr.bechtstudio.com/",
    "https://github.com/MaximBecht/klirr/",
];

fn link_allowed(url: &str) -> bool {
    LINKS
        .iter()
        .any(|&prefix| url.starts_with(prefix) || url == &prefix[..prefix.len() - 1])
}

/// Opens a link of the Help panel in the system's browser or mail program.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !link_allowed(&url) {
        return Err(format!("not a link Klirr opens: {url}"));
    }
    tauri_plugin_opener::open_url(&url, None::<&str>).map_err(|e| {
        log::error(format!("opening a link: {e}"));
        format!("could not open the link: {e}")
    })
}

/// How long the window may stay hidden. It opens hidden and the UI shows it once it has
/// sized it to the amp; if the UI fails before that, the window is shown anyway.
const SHOW_FALLBACK: Duration = Duration::from_secs(5);

fn main() {
    let app = tauri::Builder::default()
        .manage(Audio::spawn())
        .setup(|app| {
            let path = app.path();
            log::init(
                path.app_log_dir().ok().as_deref(),
                path.home_dir().ok().as_deref(),
            );
            log::info(format!(
                "Klirr {}{} on {} ({}), webview {}",
                env!("CARGO_PKG_VERSION"),
                if cfg!(feature = "asio") {
                    " with ASIO"
                } else {
                    ""
                },
                describe_os(&os_info::get()),
                std::env::consts::ARCH,
                tauri::webview_version().unwrap_or_default()
            ));
            app.manage(open_settings(app));
            if let Some(window) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(SHOW_FALLBACK);
                    if window.is_visible().is_ok_and(|visible| !visible) {
                        let _ = window.show();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            hosts,
            devices,
            buffer_sizes,
            load_config,
            save_config,
            start,
            stop,
            set_params,
            load_ir,
            measure,
            test_tone,
            hardware_setup,
            diagnostics,
            log_ui,
            save_report,
            open_link
        ])
        .build(tauri::generate_context!())
        .expect("could not start the Klirr window");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Close the devices cleanly (ASIO drivers in particular want that) before exiting.
            let _ = app.state::<Audio>().stop();
            log::exit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{link_allowed, report_name_allowed};

    #[test]
    fn reports_are_saved_under_their_own_names_only() {
        assert!(report_name_allowed("klirr-report-20261124-184307.txt"));
        assert!(!report_name_allowed("klirr-report-../../evil.txt"));
        assert!(!report_name_allowed("klirr-report-x.exe"));
        assert!(!report_name_allowed("klirr-report-a/b.txt"));
        assert!(!report_name_allowed("settings.json"));
    }

    #[test]
    fn only_the_help_panels_links_open() {
        assert!(link_allowed("mailto:support@bechtstudio.com"));
        assert!(link_allowed("mailto:support@bechtstudio.com?subject=Bug"));
        assert!(link_allowed("https://bechtstudio.com/en/work/klirr"));
        assert!(link_allowed("https://klirr.bechtstudio.com"));
        assert!(link_allowed("https://github.com/MaximBecht/klirr/releases"));
        assert!(!link_allowed("https://klirr.bechtstudio.com.example.org/"));
        assert!(!link_allowed("https://github.com/MaximBecht/klirr-fake"));
        assert!(!link_allowed("mailto:someone@example.org"));
        assert!(!link_allowed("file:///C:/Windows/System32/cmd.exe"));
    }
}
