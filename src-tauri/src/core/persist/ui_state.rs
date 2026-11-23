//! UI state in `<config_dir>/ui-state.json`: the agent panel's dock, size and open state, the font size, the last
//! conversation used, how the history list is grouped, how queued agent messages are sent, and whether the terminal's
//! command separators and agent labels are hidden, and whether follow-up prompts are suggested after each reply.
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
const DOCKS: [&str; 2] = ["left", "right"];
const HISTORY_GROUPS: [&str; 2] = ["date", "workspace"];
/// How queued agent messages are sent: all together, or one per turn.
const QUEUE_MODES: [&str; 2] = ["all", "one"];
/// How reasoning blocks are shown: always collapsed, open while thinking then collapsed, or always expanded.
const THINKING_MODES: [&str; 3] = ["collapsed", "auto", "expanded"];
const FONT_SIZES: RangeInclusive<u32> = 8..=32;
/// The separators and agent labels are hidden unless the file says otherwise.
const HIDE_MARKS_DEFAULT: bool = true;
/// Suggested prompts are on unless the file says otherwise. An explicit `false` in the file is kept.
const SUGGEST_PROMPTS_DEFAULT: bool = true;
/// Desktop notifications (agent done, or asking something, while Watt is in the background) are on unless the file says otherwise.
const NOTIFICATIONS_DEFAULT: bool = true;
/// The layout inspector (WebView2 DevTools, from the right-click menu) is off unless the file says otherwise.
const DEVTOOLS_DEFAULT: bool = false;
/// Largest accepted panel size, in pixels.
const MAX_PANEL_SIZE: f64 = 20_000.0;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PanelSizes {
    pub left: Option<f64>,
    pub right: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    /// One of `left`, `right`.
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
    /// `all` or `one`: how messages queued while the agent works are sent. Missing or invalid reads as `None` (all).
    pub queue_mode: Option<String>,
    /// `collapsed`, `auto` or `expanded`: how reasoning blocks are shown. Missing or invalid reads as `None` (auto).
    pub thinking_mode: Option<String>,
    /// Hides the terminal's command separators and agent labels (the exit-coloured stripes stay). Missing or invalid
    /// in the file reads as the default, `true`.
    #[serde(default = "hide_marks_default")]
    pub hide_marks: bool,
    /// After each reply, asks the model for a few follow-up prompts shown under the input. One extra model call per
    /// reply. Missing or invalid in the file reads as the default, `true`.
    #[serde(default = "suggest_prompts_default")]
    pub suggest_prompts: bool,
    /// System notification when the agent is done or asks something while Watt is in the background. Missing or invalid
    /// in the file reads as the default, `true`.
    #[serde(default = "notifications_default")]
    pub notifications: bool,
    /// Enables the layout inspector: the right-click menu's "Open layout inspector" item, and the command that opens
    /// DevTools. Missing or invalid in the file reads as the default, `false`.
    #[serde(default = "devtools_default")]
    pub devtools: bool,
}

fn hide_marks_default() -> bool {
    HIDE_MARKS_DEFAULT
}

fn suggest_prompts_default() -> bool {
    SUGGEST_PROMPTS_DEFAULT
}

fn notifications_default() -> bool {
    NOTIFICATIONS_DEFAULT
}

fn devtools_default() -> bool {
    DEVTOOLS_DEFAULT
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            version: UI_STATE_VERSION,
            panel: None,
            font_size: None,
            last_conversation_id: None,
            history_group_by: None,
            queue_mode: None,
            thinking_mode: None,
            hide_marks: HIDE_MARKS_DEFAULT,
            suggest_prompts: SUGGEST_PROMPTS_DEFAULT,
            notifications: NOTIFICATIONS_DEFAULT,
            devtools: DEVTOOLS_DEFAULT,
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
    #[serde(deserialize_with = "double_option")]
    pub queue_mode: Option<Option<String>>,
    #[serde(deserialize_with = "double_option")]
    pub thinking_mode: Option<Option<String>>,
    /// A boolean has no "clear", so `null` is the same as absent: the value is left alone.
    pub hide_marks: Option<bool>,
    /// As `hide_marks`: a boolean, so `null` leaves it alone.
    pub suggest_prompts: Option<bool>,
    /// As `hide_marks`: a boolean, so `null` leaves it alone.
    pub notifications: Option<bool>,
    /// As `hide_marks`: a boolean, so `null` leaves it alone.
    pub devtools: Option<bool>,
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
        if let Some(mode) = patch.queue_mode {
            state.queue_mode = mode;
        }
        if let Some(mode) = patch.thinking_mode {
            state.thinking_mode = mode;
        }
        if let Some(hide) = patch.hide_marks {
            state.hide_marks = hide;
        }
        if let Some(suggest) = patch.suggest_prompts {
            state.suggest_prompts = suggest;
        }
        if let Some(notify) = patch.notifications {
            state.notifications = notify;
        }
        if let Some(devtools) = patch.devtools {
            state.devtools = devtools;
        }
        state.version = UI_STATE_VERSION;
        self.write(&state)?;
        Ok(state)
    }

    /// Clears `last_conversation_id` when it is `id`, in one locked read and write. Writes nothing when it names another
    /// conversation or none. Returns whether it was cleared.
    pub fn clear_last_conversation_if(&self, id: &str) -> Result<bool, PersistError> {
        let _guard = self.guard();
        let mut state = self.read()?;
        if state.last_conversation_id.as_deref() != Some(id) {
            return Ok(false);
        }
        state.last_conversation_id = None;
        self.write(&state)?;
        Ok(true)
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self, state: &UiState) -> Result<(), PersistError> {
        let mut text = serde_json::to_string_pretty(state)
            .map_err(|e| PersistError::Io(format!("cannot encode {}: {e}", self.path.display())))?;
        text.push('\n');
        write_private_atomic(&self.path, text.as_bytes())
            .map_err(|e| PersistError::Io(format!("{}: {e}", self.path.display())))
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
        queue_mode: obj
            .get("queue_mode")
            .and_then(Value::as_str)
            .filter(|m| QUEUE_MODES.contains(m))
            .map(str::to_string),
        thinking_mode: obj
            .get("thinking_mode")
            .and_then(Value::as_str)
            .filter(|m| THINKING_MODES.contains(m))
            .map(str::to_string),
        hide_marks: obj.get("hide_marks").and_then(Value::as_bool).unwrap_or(HIDE_MARKS_DEFAULT),
        suggest_prompts: obj.get("suggest_prompts").and_then(Value::as_bool).unwrap_or(SUGGEST_PROMPTS_DEFAULT),
        notifications: obj.get("notifications").and_then(Value::as_bool).unwrap_or(NOTIFICATIONS_DEFAULT),
        devtools: obj.get("devtools").and_then(Value::as_bool).unwrap_or(DEVTOOLS_DEFAULT),
    }
}

/// A panel with a known dock and a boolean `open`. Each size is kept only when it is a valid size. A panel stored with
/// a dock that is no longer offered (top or bottom, before those were removed) is dropped whole, so it reads as absent
/// and the default layout applies.
fn parse_panel(value: &Value) -> Option<PanelState> {
    let obj = value.as_object()?;
    let dock = obj.get("dock")?.as_str().filter(|d| DOCKS.contains(d))?.to_string();
    let open = obj.get("open")?.as_bool()?;
    let sizes = obj.get("sizes").and_then(Value::as_object);
    let size = |key: &str| sizes.and_then(|s| s.get(key)).and_then(Value::as_f64).filter(|n| valid_size(*n));
    Some(PanelState { dock, sizes: PanelSizes { left: size("left"), right: size("right") }, open })
}

fn valid_size(n: f64) -> bool {
    n.is_finite() && n > 0.0 && n <= MAX_PANEL_SIZE
}

/// Refuses a patch with a value that could never be stored. Names the field, never the value.
fn validate_patch(patch: &UiStatePatch) -> Result<(), PersistError> {
    if let Some(Some(panel)) = &patch.panel {
        if !DOCKS.contains(&panel.dock.as_str()) {
            return Err(PersistError::Invalid("panel dock must be left or right".into()));
        }
        let sizes = [panel.sizes.left, panel.sizes.right];
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
    if let Some(Some(mode)) = &patch.queue_mode
        && !QUEUE_MODES.contains(&mode.as_str())
    {
        return Err(PersistError::Invalid("queue mode must be all or one".into()));
    }
    if let Some(Some(mode)) = &patch.thinking_mode
        && !THINKING_MODES.contains(&mode.as_str())
    {
        return Err(PersistError::Invalid("thinking mode must be collapsed, auto or expanded".into()));
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
            sizes: PanelSizes { left: Some(300.0), right: Some(220.5) },
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
            panel: Some(Some(panel("right", true))),
            font_size: Some(Some(14)),
            last_conversation_id: Some(Some(id(1))),
            history_group_by: Some(Some("workspace".into())),
            queue_mode: Some(Some("one".into())),
            thinking_mode: Some(Some("expanded".into())),
            hide_marks: Some(false),
            suggest_prompts: Some(true),
            notifications: Some(false),
            devtools: Some(true),
        };
        let written = s.patch(patch).unwrap();
        assert_eq!(s.load().unwrap(), written);
        assert_eq!(written.panel, Some(panel("right", true)));
        assert_eq!(written.font_size, Some(14));
        assert_eq!(written.last_conversation_id, Some(id(1)));
        assert_eq!(written.history_group_by.as_deref(), Some("workspace"));
        assert_eq!(written.queue_mode.as_deref(), Some("one"));
        assert_eq!(written.thinking_mode.as_deref(), Some("expanded"));
        assert!(!written.hide_marks);
        assert!(written.suggest_prompts);
        assert!(!written.notifications);
        assert!(written.devtools);
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
                "panel": {"dock": "right", "open": true, "sizes": "wide"}}"#,
        );
        assert_eq!(state.font_size, None);
        assert_eq!(state.last_conversation_id, None);
        assert_eq!(state.history_group_by, None);
        let p = state.panel.expect("the panel is valid apart from its sizes");
        assert_eq!(p.dock, "right");
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
        for dock in ["left", "right"] {
            let text = format!(r#"{{"panel": {{"dock": "{dock}", "open": false}}}}"#);
            assert_eq!(read_text(&tmp, &text).panel.unwrap().dock, dock);
        }
    }

    #[test]
    fn a_stored_top_or_bottom_dock_is_dropped_on_load() {
        // Old top and bottom docks are gone: the panel is dropped whole, so the default layout applies. The rest of the
        // file is kept.
        let tmp = TempDir::new();
        for dock in ["top", "bottom"] {
            let text = format!(
                r#"{{"font_size": 12, "panel": {{"dock": "{dock}", "open": true, "sizes": {{"left": 300, "{dock}": 200}}}}}}"#
            );
            let state = read_text(&tmp, &text);
            assert!(state.panel.is_none(), "dock {dock} must be dropped");
            assert_eq!(state.font_size, Some(12));
        }
    }

    #[test]
    fn panel_sizes_must_be_positive_and_at_most_20000() {
        let tmp = TempDir::new();
        let state = read_text(
            &tmp,
            r#"{"panel": {"dock": "left", "open": true, "sizes": {
                "left": 0, "right": 20001}}}"#,
        );
        let sizes = state.panel.unwrap().sizes;
        assert_eq!(sizes.left, None);
        assert_eq!(sizes.right, None);

        let state = read_text(&tmp, r#"{"panel": {"dock": "right", "open": true, "sizes": {"right": -5, "left": 20000}}}"#);
        let sizes = state.panel.unwrap().sizes;
        assert_eq!(sizes.left, Some(20_000.0));
        assert_eq!(sizes.right, None);

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
            .patch(patch_from(json!({"panel": {"dock": "right", "open": false, "sizes": {}}})))
            .unwrap();
        assert_eq!(state.panel, Some(PanelState { dock: "right".into(), sizes: PanelSizes::default(), open: false }));
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
            json!({"panel": {"dock": "left", "open": true, "sizes": {"right": 25000}}}),
            json!({"panel": {"dock": "top", "open": true}}),
            json!({"panel": {"dock": "bottom", "open": true}}),
            json!({"font_size": 40}),
            json!({"font_size": 7}),
            json!({"last_conversation_id": "../etc"}),
            json!({"history_group_by": "month"}),
            json!({"queue_mode": "some"}),
            json!({"thinking_mode": "open"}),
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

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
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
    fn hide_marks_defaults_to_true_and_an_old_file_without_it_loads_as_true() {
        let tmp = TempDir::new();
        assert!(store(&tmp).load().unwrap().hide_marks, "no file");
        assert!(UiState::default().hide_marks);
        // A file written before the setting existed has no hide_marks field.
        let old = read_text(&tmp, r#"{"version": 1, "font_size": 12, "history_group_by": "date"}"#);
        assert!(old.hide_marks);
        assert_eq!(old.font_size, Some(12));
    }

    #[test]
    fn hide_marks_must_be_a_boolean_or_it_reads_as_the_default() {
        let tmp = TempDir::new();
        assert!(read_text(&tmp, r#"{"hide_marks": "no"}"#).hide_marks);
        assert!(read_text(&tmp, r#"{"hide_marks": 0}"#).hide_marks);
        assert!(read_text(&tmp, r#"{"hide_marks": null}"#).hide_marks);
        assert!(!read_text(&tmp, r#"{"hide_marks": false}"#).hide_marks);
    }

    #[test]
    fn hide_marks_patch_sets_persists_and_null_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let off = s.patch(patch_from(json!({"hide_marks": false}))).unwrap();
        assert!(!off.hide_marks);
        assert!(!s.load().unwrap().hide_marks, "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert!(!untouched.hide_marks, "an absent field leaves it alone");

        let null = s.patch(patch_from(json!({"hide_marks": null}))).unwrap();
        assert!(!null.hide_marks, "null leaves it alone too");

        let on = s.patch(patch_from(json!({"hide_marks": true}))).unwrap();
        assert!(on.hide_marks);
        assert!(s.load().unwrap().hide_marks);
    }

    #[test]
    fn queue_mode_is_all_or_one_only_and_reads_as_none_otherwise() {
        let tmp = TempDir::new();
        assert_eq!(read_text(&tmp, r#"{"queue_mode": "all"}"#).queue_mode.as_deref(), Some("all"));
        assert_eq!(read_text(&tmp, r#"{"queue_mode": "one"}"#).queue_mode.as_deref(), Some("one"));
        assert_eq!(read_text(&tmp, r#"{"queue_mode": "many"}"#).queue_mode, None);
        assert_eq!(read_text(&tmp, r#"{"queue_mode": true}"#).queue_mode, None);
        assert_eq!(read_text(&tmp, r#"{"font_size": 12}"#).queue_mode, None, "an old file has none");
        assert_eq!(UiState::default().queue_mode, None);
    }

    #[test]
    fn thinking_mode_is_collapsed_auto_or_expanded_and_reads_as_none_otherwise() {
        let tmp = TempDir::new();
        for mode in ["collapsed", "auto", "expanded"] {
            assert_eq!(read_text(&tmp, &format!(r#"{{"thinking_mode": "{mode}"}}"#)).thinking_mode.as_deref(), Some(mode));
        }
        assert_eq!(read_text(&tmp, r#"{"thinking_mode": "open"}"#).thinking_mode, None);
        assert_eq!(read_text(&tmp, r#"{"thinking_mode": true}"#).thinking_mode, None);
        let kept = read_text(&tmp, r#"{"thinking_mode": "open", "queue_mode": "one"}"#);
        assert_eq!(kept.queue_mode.as_deref(), Some("one"), "an invalid thinking mode does not drop the rest");
        assert_eq!(read_text(&tmp, r#"{"font_size": 12}"#).thinking_mode, None, "an old file has none");
        assert_eq!(UiState::default().thinking_mode, None);
    }

    #[test]
    fn thinking_mode_patch_sets_clears_and_absent_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let expanded = s.patch(patch_from(json!({"thinking_mode": "expanded"}))).unwrap();
        assert_eq!(expanded.thinking_mode.as_deref(), Some("expanded"));
        assert_eq!(s.load().unwrap().thinking_mode.as_deref(), Some("expanded"), "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert_eq!(untouched.thinking_mode.as_deref(), Some("expanded"), "an absent field leaves it alone");

        let cleared = s.patch(patch_from(json!({"thinking_mode": null}))).unwrap();
        assert_eq!(cleared.thinking_mode, None, "null clears it, back to the default");
        assert_eq!(patch_from(json!({"thinking_mode": "collapsed"})).thinking_mode, Some(Some("collapsed".into())));
        assert_eq!(UiStatePatch::default().thinking_mode, None);
    }

    #[test]
    fn queue_mode_patch_sets_clears_and_absent_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let one = s.patch(patch_from(json!({"queue_mode": "one"}))).unwrap();
        assert_eq!(one.queue_mode.as_deref(), Some("one"));
        assert_eq!(s.load().unwrap().queue_mode.as_deref(), Some("one"), "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert_eq!(untouched.queue_mode.as_deref(), Some("one"), "an absent field leaves it alone");

        let cleared = s.patch(patch_from(json!({"queue_mode": null}))).unwrap();
        assert_eq!(cleared.queue_mode, None, "null clears it, back to the default");
        assert_eq!(patch_from(json!({"queue_mode": "all"})).queue_mode, Some(Some("all".into())));
        assert_eq!(UiStatePatch::default().queue_mode, None);
    }

    #[test]
    fn hide_marks_is_in_the_patch_default_as_absent() {
        assert_eq!(UiStatePatch::default().hide_marks, None);
        assert_eq!(patch_from(json!({"hide_marks": false})).hide_marks, Some(false));
    }

    #[test]
    fn suggest_prompts_is_on_by_default_and_a_boolean_only() {
        let tmp = TempDir::new();
        assert!(store(&tmp).load().unwrap().suggest_prompts, "no file");
        assert!(UiState::default().suggest_prompts);
        assert!(read_text(&tmp, r#"{"font_size": 12}"#).suggest_prompts, "an old file has none");
        assert!(read_text(&tmp, r#"{"suggest_prompts": true}"#).suggest_prompts);
        assert!(!read_text(&tmp, r#"{"suggest_prompts": false}"#).suggest_prompts, "an explicit false is kept");
        assert!(read_text(&tmp, r#"{"suggest_prompts": "yes"}"#).suggest_prompts, "non-boolean falls back to the default");
        assert!(read_text(&tmp, r#"{"suggest_prompts": 1}"#).suggest_prompts, "non-boolean falls back to the default");
    }

    #[test]
    fn suggest_prompts_patch_sets_persists_and_null_or_absent_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let on = s.patch(patch_from(json!({"suggest_prompts": true}))).unwrap();
        assert!(on.suggest_prompts);
        assert!(s.load().unwrap().suggest_prompts, "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert!(untouched.suggest_prompts, "an absent field leaves it alone");

        let null = s.patch(patch_from(json!({"suggest_prompts": null}))).unwrap();
        assert!(null.suggest_prompts, "null leaves it alone too");

        let off = s.patch(patch_from(json!({"suggest_prompts": false}))).unwrap();
        assert!(!off.suggest_prompts);
        assert_eq!(patch_from(json!({})).suggest_prompts, None);
    }

    #[test]
    fn notifications_are_on_by_default_and_a_boolean_only() {
        let tmp = TempDir::new();
        assert!(store(&tmp).load().unwrap().notifications, "no file");
        assert!(UiState::default().notifications);
        assert!(read_text(&tmp, r#"{"font_size": 12}"#).notifications, "an old file has none");
        assert!(!read_text(&tmp, r#"{"notifications": false}"#).notifications);
        assert!(read_text(&tmp, r#"{"notifications": "no"}"#).notifications);
        assert!(read_text(&tmp, r#"{"notifications": 0}"#).notifications);
        assert!(read_text(&tmp, r#"{"notifications": null}"#).notifications);
    }

    #[test]
    fn notifications_patch_sets_persists_and_null_or_absent_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let off = s.patch(patch_from(json!({"notifications": false}))).unwrap();
        assert!(!off.notifications);
        assert!(!s.load().unwrap().notifications, "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert!(!untouched.notifications, "an absent field leaves it alone");

        let null = s.patch(patch_from(json!({"notifications": null}))).unwrap();
        assert!(!null.notifications, "null leaves it alone too");

        let on = s.patch(patch_from(json!({"notifications": true}))).unwrap();
        assert!(on.notifications);
        assert!(s.load().unwrap().notifications);
        assert_eq!(patch_from(json!({})).notifications, None);
    }

    #[test]
    fn devtools_is_off_by_default_and_a_boolean_only() {
        let tmp = TempDir::new();
        assert!(!store(&tmp).load().unwrap().devtools, "no file");
        assert!(!UiState::default().devtools);
        assert!(!read_text(&tmp, r#"{"font_size": 12}"#).devtools, "an old file has none");
        assert!(read_text(&tmp, r#"{"devtools": true}"#).devtools);
        assert!(!read_text(&tmp, r#"{"devtools": "yes"}"#).devtools, "non-boolean falls back to the default");
        assert!(!read_text(&tmp, r#"{"devtools": 1}"#).devtools, "non-boolean falls back to the default");
        assert!(!read_text(&tmp, r#"{"devtools": null}"#).devtools);
    }

    #[test]
    fn devtools_patch_sets_persists_and_null_or_absent_leaves_it_alone() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let on = s.patch(patch_from(json!({"devtools": true}))).unwrap();
        assert!(on.devtools);
        assert!(s.load().unwrap().devtools, "written to the file");

        let untouched = s.patch(patch_from(json!({"font_size": 13}))).unwrap();
        assert!(untouched.devtools, "an absent field leaves it alone");

        let null = s.patch(patch_from(json!({"devtools": null}))).unwrap();
        assert!(null.devtools, "null leaves it alone too");

        let off = s.patch(patch_from(json!({"devtools": false}))).unwrap();
        assert!(!off.devtools);
        assert!(!s.load().unwrap().devtools);
        assert_eq!(patch_from(json!({})).devtools, None);
    }

    #[test]
    fn ui_state_path_is_inside_the_config_directory() {
        assert_eq!(ui_state_path(Path::new("/cfg")), PathBuf::from("/cfg/ui-state.json"));
    }
}
