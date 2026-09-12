//! Loadout slots and the shared activation-prefix chord.
//!
//! Five slots, activated by `{prefix}-1` … `{prefix}-5`. The default prefix is
//! `mod-shift` (Cmd+Shift on macOS, Ctrl+Shift elsewhere). Recording captures
//! the modifiers from a full keystroke and keeps the slot numbers.

use serde::{Deserialize, Serialize};

use zeron_proto::{HarnessId, Model, ModelOption, ReasoningLevel};

use crate::settings::{KeymapConfig, ShortcutId, combo_from_keystroke_on, platform_combo_on};

/// How many loadout slots the composer can jump between.
pub const LOADOUT_SLOTS: usize = 5;

/// Default activation prefix: Cmd+Shift on macOS, Ctrl+Shift elsewhere.
pub const DEFAULT_LOADOUT_PREFIX: &str = "mod-shift";

/// One saved loadout entry: the full run config a slot applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadoutSlot {
    pub harness: HarnessId,
    pub model: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub model_options: serde_json::Map<String, serde_json::Value>,
}

/// Persisted loadout: a shared prefix plus five slots (trailing empties allowed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoadoutConfig {
    pub prefix: String,
    pub slots: Vec<Option<LoadoutSlot>>,
}

impl Default for LoadoutConfig {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_LOADOUT_PREFIX.into(),
            slots: vec![None; LOADOUT_SLOTS],
        }
    }
}

impl LoadoutConfig {
    /// Heal a hand-edited or truncated file into a usable five-slot loadout.
    pub fn clamped(mut self) -> Self {
        if !prefix_is_valid(&self.prefix) {
            self.prefix = DEFAULT_LOADOUT_PREFIX.into();
        }
        if self.slots.len() > LOADOUT_SLOTS {
            self.slots.truncate(LOADOUT_SLOTS);
        }
        while self.slots.len() < LOADOUT_SLOTS {
            self.slots.push(None);
        }
        pack_slots(&mut self.slots);
        self
    }

    pub fn slot(&self, index: usize) -> Option<&LoadoutSlot> {
        self.slots.get(index).and_then(|slot| slot.as_ref())
    }

    /// First empty index, or `None` when the loadout is full.
    pub fn first_empty(&self) -> Option<usize> {
        self.slots.iter().position(|slot| slot.is_none())
    }

    /// Replace `index` when filled; otherwise fill the first empty slot.
    pub fn place(&mut self, index: usize, slot: LoadoutSlot) {
        if self.slot(index).is_some() && index < self.slots.len() {
            self.slots[index] = Some(slot);
            return;
        }
        if let Some(at) = self.first_empty() {
            self.slots[at] = Some(slot);
        }
        pack_slots(&mut self.slots);
    }

    pub fn remove(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = None;
        }
        pack_slots(&mut self.slots);
    }
}

/// Providers the first revision of the loadout page offers. Pi is excluded;
/// OpenRouter is not a harness in this app.
pub fn loadout_supports_harness(id: HarnessId) -> bool {
    !matches!(id, HarnessId::Pi | HarnessId::Mock)
}

/// Pack filled slots to the left so keybinds stay 1..N in visual order.
pub fn pack_slots(slots: &mut [Option<LoadoutSlot>]) {
    let filled: Vec<LoadoutSlot> = slots.iter_mut().filter_map(Option::take).collect();
    for (index, slot) in filled.into_iter().enumerate() {
        if index < slots.len() {
            slots[index] = Some(slot);
        }
    }
}

/// Whether `prefix` is a non-empty modifier-only chord (`mod-shift`, `ctrl-alt`).
pub fn prefix_is_valid(prefix: &str) -> bool {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return false;
    }
    prefix
        .split('-')
        .all(|part| matches!(part, "mod" | "ctrl" | "alt" | "shift") && !part.is_empty())
}

/// `{prefix}-{n}` for a 0-based slot.
pub fn loadout_combo(prefix: &str, slot: usize) -> String {
    format!("{}-{}", prefix, slot + 1)
}

/// Drop the trailing key from a full combo, leaving the modifier prefix.
pub fn prefix_from_combo(combo: &str) -> Option<String> {
    let mut parts: Vec<&str> = combo.split('-').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    parts.pop();
    let prefix = parts.join("-");
    prefix_is_valid(&prefix).then_some(prefix)
}

/// Outcome of one keystroke while recording the loadout prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordPrefixOutcome {
    Cancelled,
    Ignored,
    Set(String),
}

/// Record a shared prefix from a complete keystroke. The trailing key is
/// discarded so slots stay 1–N; Escape cancels; bare modifiers are ignored.
pub fn record_loadout_prefix(
    key: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
) -> RecordPrefixOutcome {
    record_loadout_prefix_on(cfg!(target_os = "macos"), key, ctrl, alt, shift, cmd)
}

pub fn record_loadout_prefix_on(
    mac: bool,
    key: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
) -> RecordPrefixOutcome {
    if key.eq_ignore_ascii_case("escape") {
        return RecordPrefixOutcome::Cancelled;
    }
    match combo_from_keystroke_on(mac, ctrl, alt, shift, cmd, key)
        .as_deref()
        .and_then(prefix_from_combo)
    {
        Some(prefix) => RecordPrefixOutcome::Set(prefix),
        None => RecordPrefixOutcome::Ignored,
    }
}

/// The shortcut (if any) that already owns one of `prefix-1` … `prefix-N`.
pub fn loadout_prefix_conflict(keymap: &KeymapConfig, prefix: &str) -> Option<ShortcutId> {
    loadout_prefix_conflict_on(cfg!(target_os = "macos"), keymap, prefix)
}

pub fn loadout_prefix_conflict_on(
    mac: bool,
    keymap: &KeymapConfig,
    prefix: &str,
) -> Option<ShortcutId> {
    if !prefix_is_valid(prefix) {
        return None;
    }
    (0..LOADOUT_SLOTS).find_map(|slot| {
        let combo = platform_combo_on(mac, &loadout_combo(prefix, slot));
        ShortcutId::ALL.into_iter().find(|&id| {
            let existing = keymap.get(id);
            !existing.is_empty() && platform_combo_on(mac, existing) == combo
        })
    })
}

/// Why a loadout shortcut could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyLoadoutError {
    EmptySlot,
    HarnessLocked {
        current: Option<HarnessId>,
        wanted: HarnessId,
    },
}

/// Gate for applying a slot: new chats always accept a filled slot; an
/// existing chat only accepts the same harness.
pub fn apply_loadout_gate(
    chat_exists: bool,
    current_harness: Option<HarnessId>,
    slot: Option<&LoadoutSlot>,
) -> Result<(), ApplyLoadoutError> {
    let Some(slot) = slot else {
        return Err(ApplyLoadoutError::EmptySlot);
    };
    if chat_exists && current_harness != Some(slot.harness) {
        return Err(ApplyLoadoutError::HarnessLocked {
            current: current_harness,
            wanted: slot.harness,
        });
    }
    Ok(())
}

pub fn apply_loadout_error_message(error: &ApplyLoadoutError) -> String {
    match error {
        ApplyLoadoutError::EmptySlot => "That loadout slot is empty.".into(),
        ApplyLoadoutError::HarnessLocked { wanted, .. } => {
            format!(
                "Can't switch to {} on an existing chat. Start a new session first.",
                harness_display_name(*wanted)
            )
        }
    }
}

pub fn harness_display_name(id: HarnessId) -> &'static str {
    match id {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Devin => "Devin",
        HarnessId::Grok => "Grok",
        HarnessId::Hermes => "Hermes",
        HarnessId::Pi => "Pi",
        HarnessId::Opencode => "OpenCode",
        HarnessId::Mock => "Mock",
    }
}

/// The advertised Fast / Priority (service-tier) option, if this model has one.
pub fn speed_option(model: &Model) -> Option<&ModelOption> {
    model.options.iter().find(|option| is_speed_option(option))
}

fn is_speed_option(option: &ModelOption) -> bool {
    let id = option.id.to_ascii_lowercase();
    let label = option.label.to_ascii_lowercase();
    id.contains("servicetier")
        || id.contains("speed")
        || label == "fast"
        || label == "priority"
        || option.choices.iter().any(is_speed_choice)
}

fn is_speed_choice(choice: &zeron_proto::ModelOptionChoice) -> bool {
    let id = choice.id.to_ascii_lowercase();
    let label = choice.label.to_ascii_lowercase();
    matches!(id.as_str(), "fast" | "priority") || matches!(label.as_str(), "fast" | "priority")
}

pub fn speed_choice_id(option: &ModelOption) -> Option<&str> {
    option
        .choices
        .iter()
        .find(|choice| is_speed_choice(choice))
        .map(|choice| choice.id.as_str())
}

pub fn speed_option_label(option: &ModelOption) -> &'static str {
    let Some(choice) = option.choices.iter().find(|choice| is_speed_choice(choice)) else {
        return "Fast";
    };
    if choice.label.to_ascii_lowercase().contains("priority") {
        "Priority"
    } else {
        "Fast"
    }
}

pub fn slot_speed_enabled(slot: &LoadoutSlot, model: Option<&Model>) -> bool {
    let Some(model) = model else {
        return false;
    };
    let Some(option) = speed_option(model) else {
        return false;
    };
    let Some(fast_id) = speed_choice_id(option) else {
        return false;
    };
    slot.model_options
        .get(&option.id)
        .and_then(|value| value.as_str())
        == Some(fast_id)
}

pub fn set_slot_speed(slot: &mut LoadoutSlot, model: Option<&Model>, on: bool) {
    let Some(model) = model else {
        return;
    };
    let Some(option) = speed_option(model) else {
        return;
    };
    let Some(fast_id) = speed_choice_id(option) else {
        return;
    };
    if on {
        slot.model_options.insert(
            option.id.clone(),
            serde_json::Value::String(fast_id.to_string()),
        );
    } else {
        slot.model_options.remove(&option.id);
    }
}

/// Compact badge for the shared prefix plus the 1–N range (`⌘⇧1–5`).
pub fn display_loadout_range(prefix: &str) -> String {
    display_loadout_range_on(cfg!(target_os = "macos"), prefix)
}

pub fn display_loadout_range_on(mac: bool, prefix: &str) -> String {
    let first = crate::settings::badge_combo_on(mac, &loadout_combo(prefix, 0));
    format!("{first}–{LOADOUT_SLOTS}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::KeymapConfig;
    use zeron_proto::{ModelOption, ModelOptionChoice};

    fn slot(harness: HarnessId, model: &str) -> LoadoutSlot {
        LoadoutSlot {
            harness,
            model: model.into(),
            label: model.into(),
            reasoning: Some(ReasoningLevel::High),
            model_options: serde_json::Map::new(),
        }
    }

    fn speed_model() -> Model {
        Model {
            id: "gpt-5.6".into(),
            label: "GPT-5.6 Sol".into(),
            description: None,
            reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::High],
            options: vec![ModelOption {
                id: "serviceTier".into(),
                label: "Service tier".into(),
                default_choice: "default".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "default".into(),
                        label: "Standard".into(),
                    },
                    ModelOptionChoice {
                        id: "fast".into(),
                        label: "Fast".into(),
                    },
                    ModelOptionChoice {
                        id: "priority".into(),
                        label: "Fast".into(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn default_prefix_is_mod_shift() {
        assert_eq!(LoadoutConfig::default().prefix, "mod-shift");
        assert_eq!(loadout_combo("mod-shift", 0), "mod-shift-1");
        assert_eq!(loadout_combo("mod-shift", 4), "mod-shift-5");
    }

    #[test]
    fn prefix_from_combo_drops_the_trailing_key() {
        assert_eq!(
            prefix_from_combo("mod-shift-2").as_deref(),
            Some("mod-shift")
        );
        assert_eq!(prefix_from_combo("mod-alt-1").as_deref(), Some("mod-alt"));
        assert_eq!(
            prefix_from_combo("ctrl-shift-a").as_deref(),
            Some("ctrl-shift")
        );
        assert_eq!(prefix_from_combo("mod-1").as_deref(), Some("mod"));
        assert!(prefix_from_combo("2").is_none());
        assert!(prefix_from_combo("mod").is_none());
        assert!(prefix_from_combo("").is_none());
        assert!(prefix_from_combo("enter").is_none());
    }

    #[test]
    fn record_prefix_uses_modifiers_and_keeps_slot_numbers() {
        assert_eq!(
            record_loadout_prefix_on(true, "2", false, false, true, true),
            RecordPrefixOutcome::Set("mod-shift".into())
        );
        assert_eq!(
            record_loadout_prefix_on(true, "a", false, true, false, true),
            RecordPrefixOutcome::Set("mod-alt".into())
        );
        assert_eq!(
            record_loadout_prefix_on(true, "escape", false, false, true, true),
            RecordPrefixOutcome::Cancelled
        );
        assert_eq!(
            record_loadout_prefix_on(true, "shift", false, false, true, false),
            RecordPrefixOutcome::Ignored
        );
        assert_eq!(
            record_loadout_prefix_on(true, "2", false, false, false, false),
            RecordPrefixOutcome::Ignored
        );
        assert_eq!(
            record_loadout_prefix_on(false, "3", true, false, true, false),
            RecordPrefixOutcome::Set("mod-shift".into())
        );
    }

    #[test]
    fn invalid_prefixes_are_rejected() {
        assert!(prefix_is_valid("mod-shift"));
        assert!(prefix_is_valid("mod"));
        assert!(prefix_is_valid("ctrl-alt"));
        assert!(!prefix_is_valid(""));
        assert!(!prefix_is_valid("mod-shift-1"));
        assert!(!prefix_is_valid("mod-2"));
        assert!(!prefix_is_valid("shift-enter"));
    }

    #[test]
    fn prefix_conflict_detects_jump_session_overlap() {
        let keymap = KeymapConfig::default();
        assert_eq!(
            loadout_prefix_conflict_on(true, &keymap, "mod"),
            Some(ShortcutId::JumpSession(0))
        );
        assert!(loadout_prefix_conflict_on(true, &keymap, "mod-shift").is_none());
        assert!(loadout_prefix_conflict_on(false, &keymap, "mod-shift").is_none());
    }

    #[test]
    fn prefix_conflict_detects_custom_shortcut_overlap() {
        let mut keymap = KeymapConfig::default();
        keymap.toggle_terminal = "mod-alt-3".into();
        assert_eq!(
            loadout_prefix_conflict_on(true, &keymap, "mod-alt"),
            Some(ShortcutId::ToggleTerminal)
        );
        keymap.toggle_terminal = "mod-j".into();
        assert!(loadout_prefix_conflict_on(true, &keymap, "mod-alt").is_none());
    }

    #[test]
    fn apply_gate_allows_new_chats_and_same_harness() {
        let filled = slot(HarnessId::Codex, "gpt-5.6");
        assert!(apply_loadout_gate(false, None, Some(&filled)).is_ok());
        assert!(apply_loadout_gate(true, Some(HarnessId::Codex), Some(&filled)).is_ok());
        assert_eq!(
            apply_loadout_gate(true, Some(HarnessId::ClaudeCode), Some(&filled)),
            Err(ApplyLoadoutError::HarnessLocked {
                current: Some(HarnessId::ClaudeCode),
                wanted: HarnessId::Codex,
            })
        );
        assert_eq!(
            apply_loadout_gate(false, None, None),
            Err(ApplyLoadoutError::EmptySlot)
        );
        assert_eq!(
            apply_loadout_gate(true, Some(HarnessId::Codex), None),
            Err(ApplyLoadoutError::EmptySlot)
        );
    }

    #[test]
    fn packing_keeps_filled_slots_left_aligned() {
        let mut config = LoadoutConfig::default();
        config.place(0, slot(HarnessId::ClaudeCode, "opus"));
        config.place(4, slot(HarnessId::Codex, "gpt"));
        assert!(config.slot(0).is_some());
        assert!(config.slot(1).is_some());
        assert!(config.slot(2).is_none());
        config.remove(0);
        assert_eq!(config.slot(0).map(|s| s.model.as_str()), Some("gpt"));
        assert!(config.slot(1).is_none());
    }

    #[test]
    fn place_on_occupied_slot_replaces() {
        let mut config = LoadoutConfig::default();
        config.place(0, slot(HarnessId::ClaudeCode, "opus"));
        config.place(0, slot(HarnessId::Codex, "gpt"));
        assert_eq!(config.slot(0).map(|s| s.model.as_str()), Some("gpt"));
        assert!(config.slot(1).is_none());
    }

    #[test]
    fn clamp_heals_truncated_and_invalid_files() {
        let healed = LoadoutConfig {
            prefix: "mod-shift-1".into(),
            slots: vec![Some(slot(HarnessId::Codex, "gpt"))],
        }
        .clamped();
        assert_eq!(healed.prefix, DEFAULT_LOADOUT_PREFIX);
        assert_eq!(healed.slots.len(), LOADOUT_SLOTS);
        assert!(healed.slot(0).is_some());
        assert!(healed.slot(1).is_none());
    }

    #[test]
    fn pi_and_mock_are_unsupported() {
        assert!(!loadout_supports_harness(HarnessId::Pi));
        assert!(!loadout_supports_harness(HarnessId::Mock));
        assert!(loadout_supports_harness(HarnessId::ClaudeCode));
        assert!(loadout_supports_harness(HarnessId::Opencode));
        assert!(loadout_supports_harness(HarnessId::Grok));
    }

    #[test]
    fn speed_option_detects_service_tier_fast() {
        let model = speed_model();
        let option = speed_option(&model).expect("service tier");
        assert_eq!(option.id, "serviceTier");
        assert_eq!(speed_choice_id(option), Some("fast"));
        assert_eq!(speed_option_label(option), "Fast");
        let mut slot = slot(HarnessId::Codex, "gpt-5.6");
        assert!(!slot_speed_enabled(&slot, Some(&model)));
        set_slot_speed(&mut slot, Some(&model), true);
        assert!(slot_speed_enabled(&slot, Some(&model)));
        set_slot_speed(&mut slot, Some(&model), false);
        assert!(!slot_speed_enabled(&slot, Some(&model)));
    }

    #[test]
    fn display_range_keeps_slot_numbers() {
        assert_eq!(display_loadout_range_on(true, "mod-shift"), "⇧⌘1–5");
        assert_eq!(
            display_loadout_range_on(false, "mod-shift"),
            "Ctrl+Shift+1–5"
        );
    }
}
