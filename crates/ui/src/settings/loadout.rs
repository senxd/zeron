//! Settings → Default models: a five-slot loadout fed by drag-and-drop from
//! the provider lists, with fixed Cmd+Shift+1…5 activation shortcuts.

use std::collections::HashMap;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, Subscription, Window, div, prelude::*, px,
};

use zeron_engine::registry::{HarnessDescriptor, descriptor_enabled};
use zeron_proto::{HarnessId, Model, ReasoningLevel};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::pickers::{
    default_model, default_reasoning, harness_brand_icon, normalize_model_rows, reasoning_label,
    visible_harnesses,
};
use crate::popover::{self, Loadable};
use crate::settings::loadout_model::{
    LOADOUT_SLOTS, LoadoutConfig, LoadoutSlot, RecordPrefixOutcome, display_loadout_range,
    harness_display_name, loadout_prefix_conflict, loadout_supports_harness, record_loadout_prefix,
    set_slot_speed, slot_speed_enabled, speed_option, speed_option_label,
};
use crate::settings::{KeymapConfig, widgets};
use crate::state::AppState;
use crate::theme::{self, Theme, ink};

#[derive(Debug, Clone)]
pub enum LoadoutEvent {
    Changed(LoadoutConfig),
    RecordingChanged(bool),
    OpenAgents,
}

#[derive(Clone)]
struct LoadoutModelDrag {
    harness: HarnessId,
    model_id: String,
    label: String,
}

struct LoadoutDragGhost {
    label: SharedString,
}

impl Render for LoadoutDragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .h(px(28.0))
            .max_w(px(200.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(px(8.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(12.0))
            .text_color(theme.text)
            .opacity(0.9)
            .child(div().min_w_0().truncate().child(self.label.clone()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotMenu {
    Closed,
    Root(usize),
    Agent(usize),
    Model(usize),
    Effort(usize),
}

impl SlotMenu {
    fn slot(self) -> Option<usize> {
        match self {
            SlotMenu::Closed => None,
            SlotMenu::Root(i) | SlotMenu::Agent(i) | SlotMenu::Model(i) | SlotMenu::Effort(i) => {
                Some(i)
            }
        }
    }
}

pub struct LoadoutPage {
    state: Entity<AppState>,
    loadout: LoadoutConfig,
    keymap: KeymapConfig,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    slot_menu: SlotMenu,
    gear_open: bool,
    recording: bool,
    conflict_notice: Option<SharedString>,
    drag_over: Option<usize>,
    hover_slot: Option<usize>,
    error: Option<SharedString>,
    search: Entity<ComposerInput>,
    focus: FocusHandle,
    load_task: Option<gpui::Task<()>>,
    _search_events: Subscription,
}

impl EventEmitter<LoadoutEvent> for LoadoutPage {}
impl Focusable for LoadoutPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LoadoutPage {
    pub fn new(
        state: Entity<AppState>,
        loadout: LoadoutConfig,
        keymap: KeymapConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::new("Search models…", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
        });
        let search_events = cx.subscribe(&search, |_this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                cx.notify();
            }
        });
        let mut page = Self {
            state,
            loadout: loadout.clamped(),
            keymap,
            harnesses: Loadable::Idle,
            models: HashMap::new(),
            slot_menu: SlotMenu::Closed,
            gear_open: false,
            recording: false,
            conflict_notice: None,
            drag_over: None,
            hover_slot: None,
            error: None,
            search,
            focus: cx.focus_handle(),
            load_task: None,
            _search_events: search_events,
        };
        page.load(cx);
        page
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn set_keymap(&mut self, keymap: KeymapConfig, cx: &mut Context<Self>) {
        self.keymap = keymap;
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        cx.emit(LoadoutEvent::Changed(self.loadout.clone()));
        cx.notify();
    }

    fn set_recording(&mut self, recording: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording == recording {
            return;
        }
        self.recording = recording;
        if recording {
            self.conflict_notice = None;
            window.focus(&self.focus, cx);
        }
        cx.emit(LoadoutEvent::RecordingChanged(recording));
        cx.notify();
    }

    fn engine(&self, cx: &App) -> Option<crate::state::EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.harnesses = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if let Loadable::Ready(list) = &page.harnesses {
                    let ids: Vec<HarnessId> =
                        catalog_columns(list).into_iter().map(|d| d.id).collect();
                    for id in ids {
                        page.ensure_models(id, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn ensure_models(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if matches!(
            self.models.get(&harness),
            Some(Loadable::Loading | Loadable::Ready(_))
        ) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.models.insert(harness, Loadable::Loading);
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::LIST_MODELS,
                    serde_json::json!({ "harness": harness }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.models.insert(
                    harness,
                    match result {
                        Ok(value) => match serde_json::from_value::<Vec<Model>>(value) {
                            Ok(models) => Loadable::Ready(normalize_model_rows(harness, models)),
                            Err(err) => Loadable::Error(err.to_string()),
                        },
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn model_for(&self, harness: HarnessId, model_id: &str) -> Option<&Model> {
        self.models
            .get(&harness)
            .and_then(Loadable::ready)
            .and_then(|models| models.iter().find(|model| model.id == model_id))
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            return;
        }
        let mods = &event.keystroke.modifiers;
        match record_loadout_prefix(
            &event.keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            RecordPrefixOutcome::Cancelled => {
                self.set_recording(false, window, cx);
            }
            RecordPrefixOutcome::Ignored => {}
            RecordPrefixOutcome::Set(_) => {
                if let Some(owner) =
                    loadout_prefix_conflict(&self.keymap, crate::settings::DEFAULT_LOADOUT_PREFIX)
                {
                    self.conflict_notice = Some(
                        format!(
                            "{} is already assigned to {}.",
                            display_loadout_range(crate::settings::DEFAULT_LOADOUT_PREFIX),
                            owner.label()
                        )
                        .into(),
                    );
                    self.set_recording(false, window, cx);
                } else {
                    self.loadout.prefix = crate::settings::DEFAULT_LOADOUT_PREFIX.into();
                    self.conflict_notice = None;
                    self.set_recording(false, window, cx);
                    self.commit(cx);
                }
            }
        }
        cx.stop_propagation();
    }

    fn drop_model(&mut self, index: usize, drag: &LoadoutModelDrag, cx: &mut Context<Self>) {
        let model = self.model_for(drag.harness, &drag.model_id);
        let reasoning = model
            .map(|model| default_reasoning(&model.reasoning_levels))
            .unwrap_or(Some(ReasoningLevel::High));
        let slot = LoadoutSlot {
            harness: drag.harness,
            model: drag.model_id.clone(),
            label: drag.label.clone(),
            reasoning,
            model_options: serde_json::Map::new(),
        };
        self.loadout.place(index, slot);
        self.drag_over = None;
        self.commit(cx);
    }

    fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.slot_menu = SlotMenu::Closed;
        self.gear_open = false;
        cx.notify();
    }

    fn change_slot_agent(&mut self, index: usize, harness: HarnessId, cx: &mut Context<Self>) {
        self.ensure_models(harness, cx);
        let model = self
            .models
            .get(&harness)
            .and_then(Loadable::ready)
            .and_then(|models| default_model(models))
            .cloned();
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(|s| s.as_mut()) {
            slot.harness = harness;
            if let Some(model) = model {
                slot.model = model.id.clone();
                slot.label = model.label.clone();
                slot.reasoning = default_reasoning(&model.reasoning_levels);
            } else {
                slot.model.clear();
                slot.label = harness_display_name(harness).into();
                slot.reasoning = Some(ReasoningLevel::High);
            }
            slot.model_options.clear();
        }
        self.slot_menu = SlotMenu::Root(index);
        self.commit(cx);
    }

    fn change_slot_model(
        &mut self,
        index: usize,
        model_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let reasoning = self
            .loadout
            .slot(index)
            .and_then(|slot| self.model_for(slot.harness, &model_id))
            .map(|model| default_reasoning(&model.reasoning_levels))
            .unwrap_or(Some(ReasoningLevel::High));
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(|s| s.as_mut()) {
            slot.model = model_id;
            slot.label = label;
            slot.reasoning = reasoning;
            slot.model_options.clear();
        }
        self.slot_menu = SlotMenu::Root(index);
        self.commit(cx);
    }
}

fn catalog_columns(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    visible_harnesses(list)
        .into_iter()
        .filter(|descriptor| {
            loadout_supports_harness(descriptor.id) && descriptor_enabled(descriptor)
        })
        .collect()
}

const MODEL_COLUMN_LIMIT: usize = 80;

fn is_openrouter_model(model: &Model) -> bool {
    model
        .id
        .split('/')
        .next()
        .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter"))
        || model
            .description
            .as_deref()
            .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter"))
}

fn filtered_catalog_models<'a>(
    models: &'a [Model],
    query: &str,
    openrouter: Option<bool>,
) -> Vec<&'a Model> {
    let query = query.trim();
    let mut rows: Vec<(usize, usize, &Model)> = models
        .iter()
        .enumerate()
        .filter(|(_, model)| openrouter.is_none_or(|wanted| is_openrouter_model(model) == wanted))
        .filter_map(|(index, model)| {
            if query.is_empty() {
                return Some((0, index, model));
            }
            let haystack = format!(
                "{} {} {}",
                model.label,
                model.id,
                model.description.as_deref().unwrap_or("")
            );
            popover::match_rank(query, &haystack).map(|rank| (rank, index, model))
        })
        .collect();
    rows.sort_by_key(|(rank, index, _)| (*rank, *index));
    rows.into_iter().map(|(_, _, model)| model).collect()
}

fn nav_row(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    value: String,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let mut row = div()
        .id(id)
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.0));
    if enabled {
        row = row.cursor_pointer().hover(|s| s.bg(theme::wash(0.06)));
    } else {
        row = row.opacity(0.45);
    }
    row.child(
        div()
            .flex_1()
            .text_color(theme.text)
            .child(SharedString::from(label)),
    )
    .child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(value)),
            )
            .when(enabled, |el| {
                el.child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(px(12.0))
                        .text_color(theme.text_muted.opacity(0.6)),
                )
            }),
    )
}

impl LoadoutPage {
    fn render_slot(
        &mut self,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let filled = self.loadout.slot(index).cloned();
        let first_empty = self.loadout.first_empty() == Some(index);
        let hovered = self.hover_slot == Some(index);
        let drag_over = self.drag_over == Some(index);
        let menu_open = self.slot_menu.slot() == Some(index);
        let mut card = div()
            .id(("loadout-slot", index))
            .relative()
            .flex_1()
            .h(px(148.0))
            .min_w(px(112.0))
            .rounded(px(10.0))
            .border_1()
            .flex()
            .flex_col()
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(10.0));

        if filled.is_some() {
            card = card
                .border_color(if menu_open {
                    theme.border_strong
                } else {
                    theme.border
                })
                .bg(ink(0.03))
                .cursor_pointer()
                .hover(|s| s.bg(ink(0.05)));
        } else {
            card = card
                .border_dashed()
                .border_color(if drag_over || first_empty {
                    theme.text.opacity(0.22)
                } else {
                    theme.border
                })
                .bg(if drag_over { ink(0.04) } else { ink(0.015) });
        }

        card = card
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_slot = hovered.then_some(index);
                cx.notify();
            }))
            .on_drag_move::<LoadoutModelDrag>(cx.listener(
                move |this, _: &gpui::DragMoveEvent<LoadoutModelDrag>, _, cx| {
                    if this.drag_over != Some(index) {
                        this.drag_over = Some(index);
                        cx.notify();
                    }
                },
            ))
            .on_drop::<LoadoutModelDrag>(cx.listener(
                move |this, drag: &LoadoutModelDrag, _, cx| {
                    this.drop_model(index, drag, cx);
                },
            ));

        if let Some(slot) = filled {
            let (icon_path, tint) = harness_brand_icon(slot.harness);
            let effort = slot
                .reasoning
                .map(reasoning_label)
                .unwrap_or("High")
                .to_ascii_uppercase();
            let show_clear = hovered || menu_open;
            card = card
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.gear_open = false;
                    this.slot_menu = if this.slot_menu == SlotMenu::Root(index) {
                        SlotMenu::Closed
                    } else {
                        SlotMenu::Root(index)
                    };
                    cx.notify();
                }))
                .when(index == 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(8.0))
                            .left(px(8.0))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(ink(0.12))
                            .text_size(px(9.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child(SharedString::from("DEFAULT")),
                    )
                })
                .when(show_clear, |el| {
                    el.child(
                        div()
                            .id(("loadout-clear", index))
                            .absolute()
                            .top(px(6.0))
                            .right(px(6.0))
                            .size(px(18.0))
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(ink(0.1)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.slot_menu = SlotMenu::Closed;
                                this.loadout.remove(index);
                                this.commit(cx);
                            }))
                            .child(
                                icon(icons::CLOSE)
                                    .size(px(11.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(
                            icon(icon_path)
                                .size(px(22.0))
                                .text_color(tint.unwrap_or(theme.text)),
                        )
                        .child(
                            div()
                                .mt(px(10.0))
                                .text_size(crate::typography::ui_rems(13.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(slot.label.clone())),
                        )
                        .child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_muted.opacity(0.7))
                                .child(SharedString::from(effort)),
                        ),
                );
        } else {
            let label = if first_empty {
                "DROP MODEL HERE"
            } else {
                "EMPTY"
            };
            card = card.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted.opacity(0.45))
                    .child(SharedString::from(label)),
            );
        }

        card = card.child(
            div()
                .absolute()
                .bottom(px(8.0))
                .right(px(10.0))
                .text_size(px(10.0))
                .text_color(theme.text_muted.opacity(0.45))
                .child(SharedString::from(format!("{}", index + 1))),
        );

        if menu_open {
            let menu = self.render_slot_menu(index, theme, cx);
            card = card.child(popover::anchored_menu_below(
                format!("loadout-slot-menu-{index}"),
                menu,
                None,
            ));
        }
        card
    }

    fn render_slot_menu(
        &mut self,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(slot) = self.loadout.slot(index).cloned() else {
            return div().into_any_element();
        };
        let model = self.model_for(slot.harness, &slot.model).cloned();
        let mut card = popover::popover_card(theme)
            .w(px(260.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.slot_menu = SlotMenu::Closed;
                cx.notify();
            }));

        match self.slot_menu {
            SlotMenu::Root(_) => {
                let effort_enabled = model
                    .as_ref()
                    .is_some_and(|m| !m.reasoning_levels.is_empty());
                card = card
                    .child(
                        nav_row(
                            theme,
                            "loadout-nav-agent",
                            "Agent",
                            harness_display_name(slot.harness).into(),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.slot_menu = SlotMenu::Agent(index);
                            cx.notify();
                        })),
                    )
                    .child(
                        nav_row(
                            theme,
                            "loadout-nav-model",
                            "Model",
                            slot.label.clone(),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(harness) = this.loadout.slot(index).map(|s| s.harness) {
                                this.ensure_models(harness, cx);
                            }
                            this.slot_menu = SlotMenu::Model(index);
                            cx.notify();
                        })),
                    )
                    .child(
                        nav_row(
                            theme,
                            "loadout-nav-effort",
                            "Effort",
                            slot.reasoning.map(reasoning_label).unwrap_or("High").into(),
                            effort_enabled,
                        )
                        .when(effort_enabled, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.slot_menu = SlotMenu::Effort(index);
                                cx.notify();
                            }))
                        }),
                    );
                if let Some(option) = model.as_ref().and_then(|m| speed_option(m)) {
                    let on = slot_speed_enabled(&slot, model.as_ref());
                    let label = speed_option_label(option);
                    card = card.child(
                        div()
                            .id(("loadout-fast", index))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::wash(0.06)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (harness, model_id) = match this.loadout.slot(index) {
                                    Some(slot) => (slot.harness, slot.model.clone()),
                                    None => return,
                                };
                                let model = this.model_for(harness, &model_id).cloned();
                                if let Some(slot) =
                                    this.loadout.slots.get_mut(index).and_then(|s| s.as_mut())
                                {
                                    set_slot_speed(slot, model.as_ref(), !on);
                                }
                                this.commit(cx);
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(theme.text)
                                    .child(SharedString::from(label)),
                            )
                            .child(widgets::toggle_switch(theme, on)),
                    );
                }
            }
            SlotMenu::Agent(_) => {
                card = card.child(self.submenu_header("Agent", index, theme, cx));
                let empty = Vec::new();
                let harnesses = catalog_columns(self.harnesses.ready().unwrap_or(&empty));
                for descriptor in harnesses {
                    let selected = descriptor.id == slot.harness;
                    let id = descriptor.id;
                    let name = harness_display_name(id);
                    card = card.child(
                        popover::menu_row(theme, selected, format!("loadout-agent-{name}"))
                            .id(SharedString::from(format!("loadout-agent-{name}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.change_slot_agent(index, id, cx);
                            }))
                            .child(SharedString::from(name)),
                    );
                }
            }
            SlotMenu::Model(_) => {
                card = card.child(self.submenu_header("Model", index, theme, cx));
                match self.models.get(&slot.harness).cloned() {
                    Some(Loadable::Ready(models)) => {
                        for (ix, model) in models.into_iter().enumerate() {
                            let selected = model.id == slot.model;
                            let model_id = model.id.clone();
                            let label = model.label.clone();
                            card = card.child(
                                popover::menu_row(
                                    theme,
                                    selected,
                                    format!("loadout-model-{index}-{ix}"),
                                )
                                .id(("loadout-model", ix))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_slot_model(
                                        index,
                                        model_id.clone(),
                                        label.clone(),
                                        cx,
                                    );
                                }))
                                .child(SharedString::from(model.label)),
                            );
                        }
                    }
                    Some(Loadable::Error(err)) => {
                        card = card.child(
                            div()
                                .px(px(8.0))
                                .py(px(8.0))
                                .text_color(theme.danger_muted)
                                .child(SharedString::from(err)),
                        );
                    }
                    _ => {
                        card = card.child(popover::skeleton_menu_rows(
                            "loadout-models",
                            theme,
                            4,
                            cx.entity_id(),
                            cx,
                        ));
                    }
                }
            }
            SlotMenu::Effort(_) => {
                card = card.child(self.submenu_header("Effort", index, theme, cx));
                let levels = model
                    .as_ref()
                    .map(|m| m.reasoning_levels.clone())
                    .unwrap_or_default();
                for (ix, level) in levels.into_iter().enumerate() {
                    let selected = slot.reasoning == Some(level);
                    card = card.child(
                        popover::menu_row(theme, selected, format!("loadout-effort-{ix}"))
                            .id(("loadout-effort", ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(slot) =
                                    this.loadout.slots.get_mut(index).and_then(|s| s.as_mut())
                                {
                                    slot.reasoning = Some(level);
                                    this.slot_menu = SlotMenu::Root(index);
                                    this.commit(cx);
                                }
                            }))
                            .child(SharedString::from(reasoning_label(level))),
                    );
                }
            }
            SlotMenu::Closed => {}
        }
        card.into_any_element()
    }

    fn submenu_header(
        &self,
        title: &'static str,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(SharedString::from(format!("loadout-back-{title}")))
            .px(px(8.0))
            .py(px(6.0))
            .mb(px(2.0))
            .rounded(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme::wash(0.06)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.slot_menu = SlotMenu::Root(index);
                cx.notify();
            }))
            .child(
                icon(icons::ALT_ARROW_LEFT)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(title)),
            )
    }

    fn render_provider_column(
        &mut self,
        descriptor: &HarnessDescriptor,
        openrouter_only: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (icon_path, tint) = harness_brand_icon(descriptor.id);
        let name = if openrouter_only {
            "OpenRouter"
        } else {
            harness_display_name(descriptor.id)
        };
        let mut column = div()
            .id(SharedString::from(format!("loadout-provider-{name}")))
            .flex()
            .flex_col()
            .w(px(176.0))
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .mb(px(8.0))
                    .child(
                        icon(icon_path)
                            .size(px(14.0))
                            .text_color(tint.unwrap_or(theme.text)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from(name)),
                    ),
            );

        if !descriptor.installed {
            let label = format!("Configure {name}");
            column = column.child(
                div()
                    .id(SharedString::from(format!("loadout-configure-{name}")))
                    .h(px(36.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(ink(0.04)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(LoadoutEvent::OpenAgents);
                        this.close_menus(cx);
                    }))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(label)),
                    ),
            );
            return column;
        }

        match self.models.get(&descriptor.id).cloned() {
            Some(Loadable::Ready(models)) => {
                let harness = descriptor.id;
                let query = self.search.read(cx).text();
                let provider_filter =
                    (descriptor.id == HarnessId::Opencode).then_some(openrouter_only);
                let filtered = filtered_catalog_models(&models, query, provider_filter);
                let total = filtered.len();
                let visible = total.min(MODEL_COLUMN_LIMIT);
                let rows: Vec<_> = filtered
                    .into_iter()
                    .take(visible)
                    .enumerate()
                    .map(|(ix, model)| {
                        let drag = LoadoutModelDrag {
                            harness,
                            model_id: model.id.clone(),
                            label: model.label.clone(),
                        };
                        let in_loadout = self.loadout.slots.iter().any(|slot| {
                            slot.as_ref().is_some_and(|slot| {
                                slot.harness == harness && slot.model == model.id
                            })
                        });
                        div()
                            .id(SharedString::from(format!("loadout-source-{name}-{ix}")))
                            .mb(px(6.0))
                            .h(px(36.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(ink(0.04)))
                            .on_drag(drag, |payload, _, _, cx| {
                                cx.stop_propagation();
                                let label = payload.label.clone().into();
                                cx.new(|_| LoadoutDragGhost { label })
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .text_color(theme.text)
                                    .child(SharedString::from(model.label.clone())),
                            )
                            .when(in_loadout, |el| {
                                el.child(
                                    icon(icons::CHECK)
                                        .size(px(12.0))
                                        .text_color(theme.text_muted),
                                )
                            })
                            .child(
                                icon(icons::DRAG_HANDLE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted.opacity(0.55)),
                            )
                    })
                    .collect();
                column = column.children(rows);
                if visible < total {
                    column = column.child(
                        div()
                            .pt(px(4.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "Showing {visible} of {total}. Search to narrow."
                            ))),
                    );
                }
            }
            Some(Loadable::Error(err)) => {
                column = column.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(err)),
                );
            }
            _ => {
                column = column.child(
                    div()
                        .h(px(36.0))
                        .rounded(px(8.0))
                        .bg(ink(0.04))
                        .border_1()
                        .border_color(theme.border),
                );
            }
        }
        column
    }

    fn render_gear_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        popover::popover_card(theme)
            .w(px(280.0))
            .p(px(10.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.recording {
                    this.set_recording(false, window, cx);
                }
                this.gear_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .px(px(6.0))
                    .pb(px(8.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Activation shortcut")),
            )
            .child(
                div()
                    .id("loadout-prefix-chip")
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .flex()
                    .justify_center()
                    .font_family(theme.font_mono.clone())
                    .text_size(crate::typography::ui_rems(12.0))
                    .border_color(theme.border)
                    .bg(theme.bg)
                    .text_color(theme.text)
                    .child(SharedString::from(display_loadout_range(
                        crate::settings::DEFAULT_LOADOUT_PREFIX,
                    ))),
            )
            .child(
                div()
                    .px(px(6.0))
                    .pt(px(8.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted.opacity(0.7))
                    .child(SharedString::from("Fixed shortcut for loadout slots 1–5.")),
            )
            .into_any_element()
    }
}

impl Render for LoadoutPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drag_over.is_some() && !cx.has_active_drag() {
            self.drag_over = None;
        }
        let theme = Theme::of(cx).clone();
        let body = match &self.harnesses {
            Loadable::Error(err) => widgets::error_strip(&theme, err.clone()).into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .mt(px(24.0))
                .text_color(theme.text_muted)
                .child(SharedString::from("Loading models…"))
                .into_any_element(),
            Loadable::Ready(list) => {
                let mut columns = Vec::new();
                for descriptor in catalog_columns(list) {
                    columns.push(self.render_provider_column(&descriptor, false, &theme, cx));
                    if descriptor.id == HarnessId::Opencode {
                        columns.push(self.render_provider_column(&descriptor, true, &theme, cx));
                    }
                }
                div()
                    .mt(px(28.0))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(20.0))
                    .children(columns)
                    .into_any_element()
            }
        };

        let mut gear = div()
            .id("loadout-gear")
            .relative()
            .size(px(24.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(ink(0.06)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.slot_menu = SlotMenu::Closed;
                this.gear_open = !this.gear_open;
                if this.gear_open {
                    window.focus(&this.focus, cx);
                } else if this.recording {
                    this.set_recording(false, window, cx);
                }
                cx.notify();
            }))
            .child(
                icon(icons::SETTINGS_MINIMALISTIC)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            );
        if self.gear_open {
            let menu = self.render_gear_menu(&theme, cx);
            gear = gear.child(popover::anchored_menu_below(
                "loadout-gear-menu",
                menu,
                None,
            ));
        }

        let slots: Vec<_> = (0..LOADOUT_SLOTS)
            .map(|index| self.render_slot(index, &theme, cx).into_any_element())
            .collect();

        div()
            .id("loadout-page")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .w_full()
                    .px(px(28.0))
                    .pt(px(32.0))
                    .pb(px(64.0))
                    .flex()
                    .flex_col()
                    .child(widgets::page_header(&theme, "Default models", None))
                    .child(
                        div()
                            .mt(px(28.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(14.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(SharedString::from("Loadout")),
                            )
                            .child(gear),
                    )
                    .child(widgets::page_subtitle(
                        &theme,
                        "Choose which models you want to use",
                    ))
                    .when_some(self.error.clone(), |el, message| {
                        el.child(widgets::error_strip(&theme, message))
                    })
                    .child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_row()
                            .items_stretch()
                            .gap(px(10.0))
                            .children(slots),
                    )
                    .child(
                        div()
                            .mt(px(24.0))
                            .h(px(36.0))
                            .max_w(px(420.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                icon(icons::MAGNIFER)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(div().flex_1().min_w_0().child(self.search.clone())),
                    )
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_catalog_is_provider_scoped_and_searchable() {
        let mut models: Vec<Model> = (0..7_000)
            .map(|index| Model {
                id: format!("openrouter/model-{index}"),
                label: format!("Model {index}"),
                description: Some("OpenRouter".into()),
                reasoning_levels: Vec::new(),
                options: Vec::new(),
            })
            .collect();
        models.push(Model {
            id: "anthropic/opus".into(),
            label: "Opus".into(),
            description: Some("Anthropic".into()),
            reasoning_levels: Vec::new(),
            options: Vec::new(),
        });

        let rows = filtered_catalog_models(&models, "model-6999", Some(true));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "openrouter/model-6999");
        assert!(filtered_catalog_models(&models, "opus", Some(true)).is_empty());
        assert_eq!(
            filtered_catalog_models(&models, "opus", Some(false)).len(),
            1
        );
    }
}
