//! Klirr desktop app: the web UI (`web/`, built in desktop mode) in the system webview, and
//! the amp on the native audio host (`klirr-native`), which gets the latency the browser
//! cannot: ASIO, WASAPI exclusive, Core Audio, JACK.
//!
//! The webview only shows the UI. It calls the commands below; the audio runs on the
//! [`audio`] controller's thread and the streams' own threads, never in the webview.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;

use klirr_dsp::Param;
use klirr_native::devices::{self, DeviceInfo};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{Manager, RunEvent, State};

use audio::{Audio, Event, MeasureReply, StartRequest, Started};

/// An audio host, for the driver menu.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostEntry {
    id: String,
    name: String,
    is_default: bool,
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
        })
        .collect()
}

/// Lists a host's devices (empty `host`: the default host). Can take a moment (ASIO loads its
/// drivers), so it runs off the main thread.
#[tauri::command(async)]
fn devices(host: String) -> Result<DeviceLists, String> {
    let list =
        devices::list((!host.is_empty()).then_some(host.as_str())).map_err(|e| e.to_string())?;
    Ok(DeviceLists {
        inputs: entries(list.inputs),
        outputs: entries(list.outputs),
    })
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

fn main() {
    let app = tauri::Builder::default()
        .manage(Audio::spawn())
        .invoke_handler(tauri::generate_handler![
            hosts, devices, start, stop, set_params, load_ir, measure
        ])
        .build(tauri::generate_context!())
        .expect("could not start the Klirr window");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Close the devices cleanly (ASIO drivers in particular want that) before exiting.
            let _ = app.state::<Audio>().stop();
        }
    });
}
