//! The desktop app's settings file: one JSON file in the app's config folder (`%APPDATA%`,
//! `~/Library/Application Support`, `~/.config`), the same format on every platform.
//!
//! - **Versioned.** `version` counts the format, not the app. Loading migrates an older file
//!   step by step ([`MIGRATIONS`]) before reading it.
//! - **Lenient.** A field that is missing or invalid takes its default; the rest of the file
//!   still counts. A file that is not JSON at all is kept beside the new one, not lost.
//! - **Lossless.** Fields this version does not know are kept and written back, so a newer
//!   app's settings survive a run of an older one. A file from a newer app is also copied
//!   aside before this one first writes over it.
//! - **Safe to interrupt.** Saving writes a temporary file and renames it over the old one.
//!
//! Settings that belong to one audio host (driver API) are kept per host, keyed by host id,
//! so a file carried to another platform simply has entries that do not apply there.
//!
//! Klirr only stores what it asks the drivers for when it opens its streams. It writes no
//! driver's or system's settings itself, so there is nothing to restore when it closes.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// File name in the config folder.
const FILE: &str = "settings.json";

/// One step from version `n + 1` to `n + 2` (the first entry migrates version 1 to 2). A
/// step works on the raw JSON object, before it is read, and must tolerate fields of later
/// versions being there already (an older app may have run in between, keeping them).
type Migration = fn(&mut Map<String, Value>);

/// Migrations, oldest first. Add one for every change to the format that an older file
/// would be read wrongly without; [`VERSION`] follows.
const MIGRATIONS: &[Migration] = &[];

/// The format this build writes.
pub const VERSION: u32 = 1 + MIGRATIONS.len() as u32;

/// Everything the desktop app remembers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// The format's version; always [`VERSION`] once loaded.
    #[serde(default)]
    pub version: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub audio: Audio,
    /// Amp parameters by name. The UI checks the values: it knows their ranges.
    #[serde(default, deserialize_with = "lenient")]
    pub params: Map<String, Value>,
    /// Fields this version does not know, kept as they were.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Audio settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audio {
    /// The host (driver API) to open, by id; empty for the platform's default.
    #[serde(default, deserialize_with = "lenient")]
    pub host: String,
    /// What was last chosen on each host, by host id.
    #[serde(default, deserialize_with = "lenient_map")]
    pub hosts: BTreeMap<String, HostSettings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The devices last chosen on one host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSettings {
    /// Input device name; empty for the host's default.
    #[serde(default, deserialize_with = "lenient")]
    pub input: String,
    /// Output device name; empty for the host's default.
    #[serde(default, deserialize_with = "lenient")]
    pub output: String,
    /// Input channel, from 0.
    #[serde(default, deserialize_with = "lenient")]
    pub channel: u32,
    /// Settings of each output device (empty name: the default output), by name.
    #[serde(default, deserialize_with = "lenient_map")]
    pub devices: BTreeMap<String, DeviceSettings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What Klirr asks a device for when it opens it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettings {
    /// Buffer size to ask for, in frames; None for the driver's own.
    #[serde(default, deserialize_with = "lenient")]
    pub buffer: Option<u32>,
    /// Sample rate to ask for, in Hz, where it is Klirr's to choose (WASAPI exclusive);
    /// None for the device's own.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub rate: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Reads a field, or its default if it does not fit.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// Reads a map, dropping the entries that do not fit (an object of other things: none).
fn lenient_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let Value::Object(map) = Value::deserialize(deserializer)? else {
        return Ok(BTreeMap::new());
    };
    Ok(map
        .into_iter()
        .filter_map(|(k, v)| T::deserialize(v).ok().map(|t| (k, t)))
        .collect())
}

/// How the file was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// There was none: the app's first run (or the file was deleted).
    New,
    /// Read as it was.
    Loaded,
    /// Read after migrating it from an older version.
    Migrated,
    /// Written by a newer app; read as far as this one understands it.
    Newer,
    /// Not readable; kept as [`Store::broken_path`], and the defaults are used.
    Broken,
}

/// Parses a file's text: the settings, and whether it was migrated or is newer.
fn parse(text: &str) -> Option<(Config, Status)> {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let found = map
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1)
        .max(1);
    let status = if found < VERSION {
        migrate(&mut map, found, MIGRATIONS);
        Status::Migrated
    } else if found > VERSION {
        Status::Newer
    } else {
        Status::Loaded
    };
    let mut config: Config = serde_json::from_value(Value::Object(map)).ok()?;
    config.version = VERSION;
    Some((config, status))
}

/// Runs the steps that take a file of version `from` to the newest one.
fn migrate(map: &mut Map<String, Value>, from: u32, steps: &[Migration]) {
    for step in steps.iter().skip(from as usize - 1) {
        step(map);
    }
}

/// The settings file.
pub struct Store {
    path: PathBuf,
    /// The version of a newer app's file, until it has been copied aside.
    newer: Option<u32>,
}

impl Store {
    /// The file in `dir`, and what it holds.
    pub fn open(dir: &Path) -> (Self, Config, Status) {
        let path = dir.join(FILE);
        let mut store = Self { path, newer: None };
        let (config, status) = match fs::read_to_string(&store.path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Config::default(), Status::New),
            Err(_) => (Config::default(), Status::Broken),
            Ok(text) => parse(&text).unwrap_or((Config::default(), Status::Broken)),
        };
        if status == Status::Broken {
            // Keep it for the user to look at; the next save would replace it.
            let _ = fs::rename(&store.path, store.broken_path());
        }
        if status == Status::Newer {
            store.newer = version_of(&store.path);
        }
        let config = Config {
            version: VERSION,
            ..config
        };
        (store, config, status)
    }

    /// Where the file is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where an unreadable file was moved to.
    pub fn broken_path(&self) -> PathBuf {
        self.path.with_extension("broken.json")
    }

    /// Writes `config` (at [`VERSION`]).
    ///
    /// # Errors
    ///
    /// If the folder or the file cannot be written.
    pub fn save(&mut self, config: &Config) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if let Some(version) = self.newer {
            // A newer app's file: keep a copy it can be restored from.
            fs::copy(
                &self.path,
                self.path.with_extension(format!("v{version}.json")),
            )?;
            self.newer = None;
        }
        let config = Config {
            version: VERSION,
            ..config.clone()
        };
        let mut text = serde_json::to_string_pretty(&config).map_err(io::Error::other)?;
        text.push('\n');
        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        // Replaces the old file in one step (on Windows too).
        fs::rename(&temp, &self.path)
    }
}

/// The version a file says it is.
fn version_of(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    u32::try_from(value.get("version")?.as_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klirr-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Value {
        json!({
            "version": 1,
            "audio": {
                "host": "asio",
                "hosts": {
                    "asio": {
                        "input": "Focusrite USB ASIO",
                        "output": "Focusrite USB ASIO",
                        "channel": 1,
                        "devices": { "Focusrite USB ASIO": { "buffer": 64 } }
                    },
                    "wasapi-exclusive": {
                        "input": "",
                        "output": "",
                        "channel": 0,
                        "devices": { "": { "buffer": 144, "rate": 96000 } }
                    }
                }
            },
            "params": { "gain": 0.6 }
        })
    }

    #[test]
    fn a_file_reads_back_as_written() {
        let (config, status) = parse(&sample().to_string()).unwrap();
        assert_eq!(status, Status::Loaded);
        assert_eq!(config.audio.host, "asio");
        let asio = &config.audio.hosts["asio"];
        assert_eq!(asio.channel, 1);
        assert_eq!(asio.devices["Focusrite USB ASIO"].buffer, Some(64));
        assert_eq!(asio.devices["Focusrite USB ASIO"].rate, None);
        let exclusive = &config.audio.hosts["wasapi-exclusive"].devices[""];
        assert_eq!(
            (exclusive.buffer, exclusive.rate),
            (Some(144), Some(96_000))
        );
        assert_eq!(serde_json::to_value(&config).unwrap(), sample());
    }

    #[test]
    fn an_invalid_field_takes_its_default_and_keeps_the_rest() {
        let mut v = sample();
        v["audio"]["hosts"]["asio"]["channel"] = json!("two");
        v["audio"]["hosts"]["asio"]["devices"]["Focusrite USB ASIO"]["buffer"] = json!(-5);
        v["audio"]["hosts"]["broken"] = json!(42);
        v["params"] = json!([1, 2]);
        let (config, _) = parse(&v.to_string()).unwrap();
        let asio = &config.audio.hosts["asio"];
        assert_eq!(asio.channel, 0);
        assert_eq!(asio.input, "Focusrite USB ASIO");
        assert_eq!(asio.devices["Focusrite USB ASIO"].buffer, None);
        assert!(!config.audio.hosts.contains_key("broken"));
        assert!(config.params.is_empty());
        assert_eq!(config.audio.host, "asio");
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let mut v = sample();
        v["theme"] = json!("dark");
        v["audio"]["hosts"]["asio"]["sampleRate"] = json!(96000);
        v["audio"]["hosts"]["asio"]["devices"]["Focusrite USB ASIO"]["clock"] = json!("spdif");
        let (config, _) = parse(&v.to_string()).unwrap();
        assert_eq!(serde_json::to_value(&config).unwrap(), v);
    }

    #[test]
    fn a_file_without_a_version_counts_as_version_1() {
        let mut v = sample();
        v.as_object_mut().unwrap().remove("version");
        let (config, status) = parse(&v.to_string()).unwrap();
        assert_eq!(status, Status::Loaded);
        assert_eq!(config.version, VERSION);
    }

    #[test]
    fn a_newer_file_is_read_as_far_as_understood() {
        let mut v = sample();
        v["version"] = json!(VERSION + 1);
        let (config, status) = parse(&v.to_string()).unwrap();
        assert_eq!(status, Status::Newer);
        assert_eq!(config.audio.host, "asio");
    }

    #[test]
    fn migrations_run_in_order_from_the_files_version() {
        fn to_2(m: &mut Map<String, Value>) {
            m.insert("steps".into(), json!("2"));
        }
        fn to_3(m: &mut Map<String, Value>) {
            let s = m["steps"].as_str().unwrap_or("").to_owned();
            m.insert("steps".into(), json!(s + "3"));
        }
        let steps: &[Migration] = &[to_2, to_3];
        let mut map = Map::new();
        migrate(&mut map, 1, steps);
        assert_eq!(map["steps"], "23");
        let mut map = Map::new();
        map.insert("steps".into(), json!("x"));
        migrate(&mut map, 2, steps);
        assert_eq!(map["steps"], "x3");
    }

    #[test]
    fn non_json_is_broken() {
        assert!(parse("{ not json").is_none());
        assert!(parse("[1, 2]").is_none());
    }

    #[test]
    fn the_store_saves_and_loads() {
        let dir = temp_dir("roundtrip");
        let (mut store, config, status) = Store::open(&dir);
        assert_eq!(status, Status::New);
        assert_eq!(config.version, VERSION);
        let (config, _) = parse(&sample().to_string()).unwrap();
        store.save(&config).unwrap();
        let (_, loaded, status) = Store::open(&dir);
        assert_eq!(status, Status::Loaded);
        assert_eq!(loaded, config);
        assert!(!dir.join("settings.json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_file_is_kept_aside() {
        let dir = temp_dir("broken");
        fs::write(dir.join(FILE), "{ not json").unwrap();
        let (store, config, status) = Store::open(&dir);
        assert_eq!(status, Status::Broken);
        assert_eq!(
            config,
            Config {
                version: VERSION,
                ..Config::default()
            }
        );
        assert_eq!(
            fs::read_to_string(store.broken_path()).unwrap(),
            "{ not json"
        );
        assert!(!dir.join(FILE).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_newer_file_is_copied_aside_before_the_first_save() {
        let dir = temp_dir("newer");
        let mut v = sample();
        let newer = VERSION + 1;
        v["version"] = json!(newer);
        fs::write(dir.join(FILE), v.to_string()).unwrap();
        let (mut store, config, status) = Store::open(&dir);
        assert_eq!(status, Status::Newer);
        store.save(&config).unwrap();
        let copy = dir.join(format!("settings.v{newer}.json"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), v.to_string());
        // Only once: the second save leaves the copy alone.
        fs::write(&copy, "kept").unwrap();
        store.save(&config).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "kept");
        fs::remove_dir_all(&dir).unwrap();
    }
}
