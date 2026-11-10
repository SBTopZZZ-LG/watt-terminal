//! UI state in `<config_dir>/ui-state.json`: the agent panel's dock, size and open state, the font size, the last
//! conversation used, and how the history list is grouped.
//!
//! Reading is tolerant. Unknown fields are ignored, and each field that is missing or invalid becomes `None` on its
//! own, so one bad value never loses the rest. A file that is not JSON at all reads as the default state, and it is
//! left on disk until the next `patch`, which writes a fresh file. A missing file is the default state too.
//!
//! Writing is a patch: `patch` reads the current state, applies the fields that are present in the patch, and writes
//! the result atomically. A patch field that is absent leaves the value alone, `null` clears it, and a value sets it.
//! A patch value that is out of range is refused, and nothing is written.

use std::fs;
use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::PersistError;
use super::conversations::is_valid_id;
use crate::core::settings::write_private_atomic;

pub const UI_STATE_VERSION: u32 = 1;
pub const UI_STATE_FILE: &str = "ui-state.json";
const DOCKS: [&str; 4] = ["left", "top", "right", "bottom"];
const HISTORY_GROUPS: [&str; 2] = ["date", "workspace"];
const FONT_SIZES: RangeInclusive<u32> = 8..=32;
/// Largest accepted panel size, in pixels.
const MAX_PANEL_SIZE: f64 = 20_000.0;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PanelSizes {
    pub left: Option<f64>,
    pub right: Option<f64>,
    pub top: Option<f64>,
    pub bottom: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    /// One of `left`, `top`, `right`, `bottom`.
    pub dock: String,
    #[serde(default)]
    pub sizes: PanelSizes,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub version: u32,
    pub panel: Option<PanelState>,
    pub font_size: Option<u32>,
    pub last_conversation_id: Option<String>,
    /// `date` or `workspace`.
    pub history_group_by: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            version: UI_STATE_VERSION,
            panel: None,
            font_size: None,
            last_conversation_id: None,
            history_group_by: None,
        }
    }
}

/// A change to the UI state. For each field: `None` leaves it alone, `Some(None)` clears it, `Some(Some(v))` sets it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UiStatePatch {
    #[serde(deserialize_with = "double_option")]
    pub panel: Option<Option<PanelState>>,
    #[serde(deserialize_with = "double_option")]
    pub font_size: Option<Option<u32>>,
    #[serde(deserialize_with = "double_option")]
    pub last_conversation_id: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    pub history_group_by: Option<Option<String>>,
}

/// Distinguishes an absent field (outer `None`, via `default`) from `null` (`Some(None)`).
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

/// The UI state file. Every read and write holds one lock, so two patches cannot lose each other's fields.
#[derive(Debug)]
pub struct UiStateStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl UiStateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), lock: Mutex::new(()) }
    }

    /// The state on disk, tolerantly read (see the module doc). Only an unreadable file (not a missing one) is an
    /// error.
    pub fn load(&self) -> Result<UiState, PersistError> {
        let _guard = self.guard();
        self.read()
    }

    /// Applies `patch` to the stored state, writes the result atomically, and returns it. A value out of range is
    /// refused with `Invalid`, and nothing is written.
    pub fn patch(&self, patch: UiStatePatch) -> Result<UiState, PersistError> {
        validate_patch(&patch)?;
        let _guard = self.guard();
        let mut state = self.read()?;
        if let Some(panel) = patch.panel {
            state.panel = panel;
        }
        if let Some(font_size) = patch.font_size {
            state.font_size = font_size;
        }
        if let Some(id) = patch.last_conversation_id {
            state.last_conversation_id = id;
        }
        if let Some(group) = patch.history_group_by {
            state.history_group_by = group;
        }
        state.version = UI_STATE_VERSION;

        let mut text = serde_json::to_string_pretty(&state)
            .map_err(|e| PersistError::Io(format!("cannot encode {}: {e}", self.path.display())))?;
        text.push('\n');
        write_private_atomic(&self.path, text.as_bytes())
            .map_err(|e| PersistError::Io(format!("{}: {e}", self.path.display())))?;
        Ok(state)
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read(&self) -> Result<UiState, PersistError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(UiState::default()),
            Err(e) => return Err(PersistError::Io(format!("{}: {e}", self.path.display()))),
        };
        // A file that is not JSON is treated as absent. It is replaced by the next patch.
        Ok(match serde_json::from_str::<Value>(&text) {
            Ok(value) => tolerant(&value),
            Err(_) => UiState::default(),
        })
    }
}

/// The state described by a parsed file. Each field is taken on its own and dropped to `None` when it does not hold
/// a valid value.
fn tolerant(value: &Value) -> UiState {
    let Some(obj) = value.as_object() else {
        return UiState::default();
    };
    UiState {
        version: UI_STATE_VERSION,
        panel: obj.get("panel").and_then(parse_panel),
        font_size: obj
            .get("font_size")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| FONT_SIZES.contains(n)),
        last_conversation_id: obj
            .get("last_conversation_id")
            .and_then(Value::as_str)
            .filter(|id| is_valid_id(id))
            .map(str::to_string),
        history_group_by: obj
            .get("history_group_by")
            .and_then(Value::as_str)
            .filter(|g| HISTORY_GROUPS.contains(g))
            .map(str::to_string),
    }
}

/// A panel with a known dock and a boolean `open`. Each size is kept only when it is a valid size.
fn parse_panel(value: &Value) -> Option<PanelState> {
    let obj = value.as_object()?;
    let dock = obj.get("dock")?.as_str().filter(|d| DOCKS.contains(d))?.to_string();
    let open = obj.get("open")?.as_bool()?;
    let sizes = obj.get("sizes").and_then(Value::as_object);
    let size = |key: &str| sizes.and_then(|s| s.get(key)).and_then(Value::as_f64).filter(|n| valid_size(*n));
    Some(PanelState {
        dock,
        sizes: PanelSizes { left: size("left"), right: size("right"), top: size("top"), bottom: size("bottom") },
        open,
    })
}

fn valid_size(n: f64) -> bool {
    n.is_finite() && n > 0.0 && n <= MAX_PANEL_SIZE
}

/// Refuses a patch with a value that could never be stored. Names the field, never the value.
fn validate_patch(patch: &UiStatePatch) -> Result<(), PersistError> {
    if let Some(Some(panel)) = &patch.panel {
        if !DOCKS.contains(&panel.dock.as_str()) {
            return Err(PersistError::Invalid("panel dock must be left, top, right or bottom".into()));
        }
        let sizes = [panel.sizes.left, panel.sizes.right, panel.sizes.top, panel.sizes.bottom];
        if sizes.iter().flatten().any(|n| !valid_size(*n)) {
            return Err(PersistError::Invalid(format!("panel sizes must be above 0 and at most {MAX_PANEL_SIZE}")));
        }
    }
    if let Some(Some(size)) = patch.font_size
        && !FONT_SIZES.contains(&size)
    {
        return Err(PersistError::Invalid(format!(
            "font size must be between {} and {}",
            FONT_SIZES.start(),
            FONT_SIZES.end()
        )));
    }
    if let Some(Some(id)) = &patch.last_conversation_id
        && !is_valid_id(id)
    {
        return Err(PersistError::Invalid("the last conversation id is not valid".into()));
    }
    if let Some(Some(group)) = &patch.history_group_by
        && !HISTORY_GROUPS.contains(&group.as_str())
    {
        return Err(PersistError::Invalid("history grouping must be date or workspace".into()));
    }
    Ok(())
}

/// The path of `ui-state.json` in a config directory.
pub fn ui_state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(UI_STATE_FILE)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    fn id(n: u32) -> String {
        format!("ses_{n:032x}")
    }

    fn store(tmp: &TempDir) -> UiStateStore {
        UiStateStore::new(tmp.path().join("config").join(UI_STATE_FILE))
    }

    fn panel(dock: &str, open: bool) -> PanelState {
        PanelState {
            dock: dock.into(),
            sizes: PanelSizes { left: Some(300.0), right: None, top: None, bottom: Some(220.5) },
            open,
        }
    }

    /// Writes `text` as the state file and returns the parsed state.
    fn read_text(tmp: &TempDir, text: &str) -> UiState {
        let s = store(tmp);
        fs::create_dir_all(s.path.parent().unwrap()).unwrap();
        fs::write(&s.path, text).unwrap();
        s.load().unwrap()
    }

    fn patch_from(value: Value) -> UiStatePatch {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn missing_file_loads_as_the_default_state() {
        let tmp = TempDir::new();
        let state = store(&tmp).load().unwrap();
        assert_eq!(state, UiState::default());
        assert_eq!(state.version, UI_STATE_VERSION);
        assert!(state.panel.is_none() && state.font_size.is_none());
    }

    #[test]
    fn a_full_state_round_trips_through_a_patch() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let patch = UiStatePatch {
            panel: Some(Some(panel("bottom", true))),
            font_size: Some(Some(14)),
            last_conversation_id: Some(Some(id(1))),
            history_group_by: Some(Some("workspace".into())),
        };
        let written = s.patch(patch).unwrap();
        assert_eq!(s.load().unwrap(), written);
        assert_eq!(written.panel, Some(panel("bottom", true)));
        assert_eq!(written.font_size, Some(14));
        assert_eq!(written.last_conversation_id, Some(id(1)));
        assert_eq!(written.history_group_by.as_deref(), Some("workspace"));
    }

    #[test]
    fn unknown_fields_are_ignored_at_every_level() {
        let state = read_text(
            &TempDir::new(),
            r#"{"version": 1, "future": [1, 2], "font_size": 12, "panel": {"dock": "left", "open": false,
                "color": "teal", "sizes": {"left": 200, "extra": 9}}}"#,
        );
        assert_eq!(state.font_size, Some(12));
        let p = state.panel.unwrap();
        assert_eq!(p.dock, "left");
        assert!(!p.open);
        assert_eq!(p.sizes.left, Some(200.0));
    }

    #[test]
    fn a_field_of_the_wrong_type_is_dropped_and_the_rest_is_kept() {
        let tmp = TempDir::new();
        let state = read_text(
            &tmp,
            r#"{"font_size": "big", "last_conversation_id": 5, "history_group_by": true,
                "panel": {"dock": "top", "open": true, "sizes": "wide"}}"#,
        );
        assert_eq!(state.font_size, None);
        assert_eq!(state.last_conversation_id, None);
        assert_eq!(state.history_group_by, None);
        let p = state.panel.expect("the panel is valid apart from its sizes");
        assert_eq!(p.dock, "top");
        assert_eq!(p.sizes, PanelSizes::default());
    }

    #[test]
    fn a_panel_needs_a_known_dock_and_a_boolean_open() {
        let tmp = TempDir::new();
        assert!(read_text(&tmp, r#"{"panel": {"dock": "middle", "open": true}}"#).panel.is_none());
        assert!(read_text(&tmp, r#"{"panel": {"dock": "left", "open": "yes"}}"#).panel.is_none());
        assert!(read_text(&tmp, r#"{"panel": {"dock": "left"}}"#).panel.is_none());
        assert!(read_text(&tmp, r#"{"panel": {"dock": 1, "open": true}}"#).panel.is_none());
        assert!(read_text(&tmp, r#"{"panel": "left"}"#).panel.is_none());
        for dock in ["left", "top", "right", "bottom"] {
            let text = format!(r#"{{"panel": {{"dock": "{dock}", "open": false}}}}"#);
            assert_eq!(read_text(&tmp, &text).panel.unwrap().dock, dock);
        }
    }

    #[test]
    fn panel_sizes_must_be_positive_and_at_most_20000() {
        let tmp = TempDir::new();
        let state = read_text(
            &tmp,
            r#"{"panel": {"dock": "left", "open": true, "sizes": {
                "left": 0, "right": -5, "top": 20001, "bottom": 20000}}}"#,
        );
        let sizes = state.panel.unwrap().sizes;
        assert_eq!(sizes.left, None);
        assert_eq!(sizes.right, None);
        assert_eq!(sizes.top, None);
        assert_eq!(sizes.bottom, Some(20_000.0));

        let state = read_text(&tmp, r#"{"panel": {"dock": "left", "open": true, "sizes": {"left": 0.5}}}"#);
        assert_eq!(state.panel.unwrap().sizes.left, Some(0.5));
    }

    #[test]
    fn font_size_must_be_between_8_and_32() {
        let tmp = TempDir::new();
        let cases = [("7", None), ("8", Some(8)), ("32", Some(32)), ("33", None), ("12.5", None), ("-1", None)];
        for (text, expected) in cases {
            let state = read_text(&tmp, &format!(r#"{{"font_size": {text}}}"#));
            assert_eq!(state.font_size, expected, "font_size {text}");
        }
        // Too large for u32 must not wrap around into the range.
        let state = read_text(&tmp, r#"{"font_size": 4294967304}"#);
        assert_eq!(state.font_size, None);
    }

    #[test]
    fn last_conversation_id_must_be_a_valid_id() {
        let tmp = TempDir::new();
        assert_eq!(read_text(&tmp, &format!(r#"{{"last_conversation_id": "{}"}}"#, id(9))).last_conversation_id, Some(id(9)));
        for bad in ["../secret", "/etc/passwd", "ses_XYZ", ""] {
            let state = read_text(&tmp, &format!(r#"{{"last_conversation_id": "{bad}"}}"#));
            assert_eq!(state.last_conversation_id, None, "{bad:?}");
        }
    }

    #[test]
    fn history_grouping_is_date_or_workspace_only() {
        let tmp = TempDir::new();
        assert_eq!(read_text(&tmp, r#"{"history_group_by": "date"}"#).history_group_by.as_deref(), Some("date"));
        assert_eq!(read_text(&tmp, r#"{"history_group_by": "workspace"}"#).history_group_by.as_deref(), Some("workspace"));
        assert_eq!(read_text(&tmp, r#"{"history_group_by": "month"}"#).history_group_by, None);
    }

    #[test]
    fn corrupt_json_reads_as_default_and_is_not_overwritten_until_a_patch() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        fs::create_dir_all(s.path.parent().unwrap()).unwrap();
        fs::write(&s.path, b"{ this is not json").unwrap();

        assert_eq!(s.load().unwrap(), UiState::default());
        assert_eq!(fs::read(&s.path).unwrap(), b"{ this is not json", "a read changes nothing");

        let written = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert_eq!(written.font_size, Some(13));
        let on_disk: Value = serde_json::from_str(&fs::read_to_string(&s.path).unwrap()).unwrap();
        assert_eq!(on_disk["font_size"], 13);
        assert_eq!(on_disk["version"], 1);
    }

    #[test]
    fn an_absent_patch_field_leaves_the_value_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.patch(patch_from(json!({"font_size": 14, "last_conversation_id": id(2)}))).unwrap();
        let after = s.patch(patch_from(json!({"history_group_by": "date"}))).unwrap();
        assert_eq!(after.font_size, Some(14));
        assert_eq!(after.last_conversation_id, Some(id(2)));
        assert_eq!(after.history_group_by.as_deref(), Some("date"));
    }

    #[test]
    fn an_explicit_null_clears_a_value() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.patch(patch_from(json!({
            "font_size": 14,
            "last_conversation_id": id(3),
            "history_group_by": "workspace",
            "panel": {"dock": "right", "open": true, "sizes": {"right": 400}},
        })))
        .unwrap();
        let cleared = s
            .patch(patch_from(json!({"last_conversation_id": null, "font_size": null, "panel": null})))
            .unwrap();
        assert_eq!(cleared.last_conversation_id, None);
        assert_eq!(cleared.font_size, None);
        assert_eq!(cleared.panel, None);
        assert_eq!(cleared.history_group_by.as_deref(), Some("workspace"));
        assert_eq!(s.load().unwrap(), cleared);
    }

    #[test]
    fn the_patch_deserializes_absent_null_and_value_differently() {
        let absent = patch_from(json!({}));
        assert_eq!(absent, UiStatePatch::default());
        let null = patch_from(json!({"font_size": null}));
        assert_eq!(null.font_size, Some(None));
        assert_eq!(null.panel, None);
        let value = patch_from(json!({"font_size": 9}));
        assert_eq!(value.font_size, Some(Some(9)));
    }

    #[test]
    fn a_panel_patch_can_leave_sizes_unset() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let state = s
            .patch(patch_from(json!({"panel": {"dock": "top", "open": false, "sizes": {}}})))
            .unwrap();
        assert_eq!(state.panel, Some(PanelState { dock: "top".into(), sizes: PanelSizes::default(), open: false }));
    }

    #[test]
    fn out_of_range_patch_values_are_refused_and_nothing_is_written() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.patch(patch_from(json!({"font_size": 12}))).unwrap();
        let before = fs::read(&s.path).unwrap();

        let bad = [
            json!({"panel": {"dock": "middle", "open": true}}),
            json!({"panel": {"dock": "left", "open": true, "sizes": {"left": 0}}}),
            json!({"panel": {"dock": "left", "open": true, "sizes": {"bottom": 25000}}}),
            json!({"font_size": 40}),
            json!({"font_size": 7}),
            json!({"last_conversation_id": "../etc"}),
            json!({"history_group_by": "month"}),
        ];
        for value in bad {
            let result = s.patch(patch_from(value.clone()));
            assert!(matches!(result, Err(PersistError::Invalid(_))), "{value} must be refused");
        }
        assert_eq!(fs::read(&s.path).unwrap(), before);
        assert_eq!(s.load().unwrap().font_size, Some(12));
    }

    #[test]
    fn a_refused_patch_error_does_not_echo_the_value() {
        let tmp = TempDir::new();
        let err = store(&tmp).patch(patch_from(json!({"last_conversation_id": "sk-proj-abcdefghijklmnopqrstuvwx"}))).unwrap_err();
        assert!(!err.to_string().contains("sk-proj"), "{err}");
    }

    #[test]
    fn the_state_file_is_mode_0600() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.patch(patch_from(json!({"font_size": 11}))).unwrap();
        let mode = fs::metadata(&s.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_missing_config_directory_is_created_by_the_first_patch() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        assert!(!s.path.parent().unwrap().exists());
        s.patch(patch_from(json!({"font_size": 11}))).unwrap();
        assert!(s.path.exists());
    }

    #[test]
    fn ui_state_path_is_inside_the_config_directory() {
        assert_eq!(ui_state_path(Path::new("/cfg")), PathBuf::from("/cfg/ui-state.json"));
    }
}
