//! Pure composition-editor presentation. The caller owns modal admission, IME
//! filtering, draft mutations, and thumbnail workers; this module performs no I/O.

use crate::book_composition::{ReadingDirection, ScreenSlot, SinglePlacement};
use egui::{Color32, RichText, Sense};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorPhase {
    Preparing,
    Editing,
    Saving,
    Reloading,
    Blocked,
}

#[derive(Clone, Debug)]
pub struct EditorPageView {
    /// Stable within this draft; never an items index from another viewer.
    pub id: usize,
    pub name: String,
    pub physical_number: usize,
    pub hidden: bool,
    pub missing: bool,
    pub excluded_by_policy: bool,
    /// Already uploaded by the owning context. Excluded pages never request one.
    pub thumbnail: Option<(egui::TextureId, egui::Vec2)>,
}

#[derive(Clone, Copy, Debug)]
pub enum EditorUnitView {
    Pair {
        first: usize,
        second: usize,
    },
    Single {
        page: usize,
        placement: SinglePlacement,
        companion: EditorCompanionView,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCompanionView {
    Vacant,
    Blank,
    Cover(usize),
}

#[derive(Clone, Debug)]
pub struct EditorView {
    pub book_name: String,
    pub saved: bool,
    pub phase: EditorPhase,
    pub direction: ReadingDirection,
    pub cover: Option<usize>,
    /// Page ids equal offsets in this array, assembled by the draft adapter.
    pub pages: Vec<EditorPageView>,
    pub units: Vec<EditorUnitView>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_save: bool,
    pub can_remove: bool,
    pub landing: String,
    pub error: Option<String>,
    /// The draft owns the proposed operation. Rendering never applies it.
    pub pending_preview: Option<EditorChangePreview>,
}

#[derive(Clone, Debug)]
pub struct EditorChangePreview {
    pub explanation: String,
    pub changed_units: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorPanel {
    Sources,
    #[default]
    Units,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorConfirmation {
    #[default]
    None,
    Remove,
    Rebuild,
}

#[derive(Clone, Debug, Default)]
pub struct EditorUiState {
    pub selected_pages: Vec<usize>,
    pub selected_unit: Option<usize>,
    pub panel: EditorPanel,
    pub confirmation: EditorConfirmation,
}

/// Only IME-filtered dialog helpers may supply these values.
#[derive(Clone, Copy, Debug, Default)]
pub struct DialogKeys {
    pub enter: bool,
    pub escape: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorUiAction {
    InsertBlank {
        page: usize,
        placement: SinglePlacement,
    },
    SetDirection(ReadingDirection),
    SetCover(Option<usize>),
    SetHidden {
        page: usize,
        hidden: bool,
    },
    MovePage {
        page: usize,
        before: usize,
    },
    MovePageStep {
        page: usize,
        forward: bool,
    },
    /// `to` is the final unit index, after removing the source unit.
    MoveUnit {
        unit: usize,
        to: usize,
    },
    Pair {
        first: usize,
        second: usize,
    },
    SplitUnit(usize),
    SetPlacement {
        unit: usize,
        placement: SinglePlacement,
    },
    SetCompanion {
        unit: usize,
        companion: EditorCompanionView,
    },
    Undo,
    Redo,
    Rebuild,
    Reload,
    IgnoreComposition,
    ConfirmEdit,
    DiscardEdit,
    Save,
    Cancel,
    Remove,
    RequestThumbnails(Vec<usize>),
}

#[derive(Clone, Copy)]
enum DragItem {
    Page(usize),
    Unit(usize),
}

/// Reusable resizable, draggable window. The owner must call it only in its
/// viewport and keep background keyboard/wheel/open admission blocked.
pub fn show_window(
    ctx: &egui::Context,
    id: egui::Id,
    view: &EditorView,
    state: &mut EditorUiState,
    keys: DialogKeys,
) -> Vec<EditorUiAction> {
    let screen = ctx.content_rect();
    let margin = 12.0;
    let max_size =
        (screen.size() - egui::vec2(margin * 2.0, margin * 2.0)).max(egui::vec2(240.0, 140.0));
    let mut open = true;
    let mut actions = Vec::new();
    ctx.memory_mut(|memory| {
        memory.set_modal_layer(egui::LayerId::new(egui::Order::Foreground, id))
    });
    egui::Window::new("本の構成を編集")
        .id(id)
        .order(egui::Order::Foreground)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(screen.min + egui::vec2(margin, margin))
        .default_size(max_size.min(egui::vec2(980.0, 580.0)))
        .min_size(egui::vec2(240.0, 140.0))
        .max_size(max_size)
        .constrain_to(screen.shrink(4.0))
        .show(ctx, |ui| {
            ui.set_max_height((screen.bottom() - ui.min_rect().top() - margin).max(80.0));
            actions = render(ui, view, state);
        });
    let can_close = !matches!(view.phase, EditorPhase::Saving | EditorPhase::Reloading);
    if (!open || keys.escape) && can_close {
        if state.confirmation != EditorConfirmation::None {
            state.confirmation = EditorConfirmation::None;
        } else if !actions.contains(&EditorUiAction::Cancel) {
            actions.push(EditorUiAction::Cancel);
        }
    }
    if keys.enter
        && view.phase == EditorPhase::Editing
        && view.can_save
        && view.pending_preview.is_none()
        && state.confirmation == EditorConfirmation::None
        && !ctx.memory(|m| m.focused().is_some())
        && actions
            .iter()
            .all(|a| matches!(a, EditorUiAction::RequestThumbnails(_)))
    {
        actions.push(EditorUiAction::Save);
    }
    actions
}

/// Render within a bounded UI. Only the body scrolls, so Save/Cancel/Remove stay
/// reachable at small screen sizes and after resizing. Index lookups are O(1).
pub fn render(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
) -> Vec<EditorUiAction> {
    let mut actions = Vec::new();
    let mut thumbnails = Vec::new();
    let editing = view.phase == EditorPhase::Editing;
    let row_height =
        ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
        );
    // The body and separator each advance the vertical layout by item_spacing.
    // Reserve both gaps as well as the separator's actual six-point height.
    let footer_height = row_height + 2.0 * ui.spacing().item_spacing.y + 6.0;
    let body_height = (ui.available_height() - footer_height).max(24.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), body_height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.floating_allocated_width = scroll.bar_inner_margin + scroll.bar_width;
            egui::ScrollArea::vertical()
                .id_salt("composition_body")
                .max_height(body_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.heading(&view.book_name);
                    ui.weak(if view.saved {
                        "保存済みの構成"
                    } else {
                        "新しい構成"
                    });
                    if let Some(error) = &view.error {
                        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
                    }
                    match view.phase {
                        EditorPhase::Preparing => {
                            ui.label("本のページを準備しています");
                        }
                        EditorPhase::Saving => {
                            ui.label("構成を保存しています");
                        }
                        EditorPhase::Reloading => {
                            ui.label("保存した構成で本を読み直しています");
                        }
                        EditorPhase::Blocked => {
                            ui.label("構成を読み取れません。保存データを保持しています。");
                            if ui.button("読み直す").clicked() {
                                actions.push(EditorUiAction::Reload);
                            }
                            if ui.button("今回は構成を使わず開く").clicked() {
                                actions.push(EditorUiAction::IgnoreComposition);
                            }
                        }
                        EditorPhase::Editing => {
                            if let Some(preview) = &view.pending_preview {
                                let prompt = egui::Frame::group(ui.style()).show(ui, |ui| {
                                    ui.label(&preview.explanation);
                                    let row = ui.spacing().interact_size.y;
                                    egui::ScrollArea::vertical()
                                        .id_salt("composition_edit_preview")
                                        .max_height(row * 4.0)
                                        .auto_shrink([false, true])
                                        .show_rows(
                                            ui,
                                            row,
                                            preview.changed_units.len(),
                                            |ui, range| {
                                                for index in range {
                                                    ui.add_sized(
                                                        egui::vec2(ui.available_width(), row),
                                                        egui::Label::new(
                                                            &preview.changed_units[index],
                                                        )
                                                        .truncate(),
                                                    );
                                                }
                                            },
                                        );
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.button("変更する").clicked() {
                                            actions.push(EditorUiAction::ConfirmEdit);
                                        }
                                        if ui.button("変更しない").clicked() {
                                            actions.push(EditorUiAction::DiscardEdit);
                                        }
                                    });
                                });
                                ui.scroll_to_rect(prompt.response.rect, Some(egui::Align::Min));
                            }
                            if view.error.is_some() && ui.button("ページを読み直す").clicked()
                            {
                                actions.push(EditorUiAction::Reload);
                            }
                            ui.add_enabled_ui(
                                view.pending_preview.is_none()
                                    && state.confirmation == EditorConfirmation::None,
                                |ui| settings(ui, view, state, &mut actions),
                            );
                            if state.confirmation != EditorConfirmation::None {
                                confirmation(ui, state, &mut actions);
                            }
                            ui.label(
                                "白紙は見開きの組だけに使います。単ページ表示では飛ばします。",
                            );
                            ui.add_enabled_ui(
                                view.pending_preview.is_none()
                                    && state.confirmation == EditorConfirmation::None,
                                |ui| selected_controls(ui, view, state, &mut actions),
                            );
                            ui.separator();
                            // Two independent virtual lists on wide screens. On narrow
                            // screens tabs preserve their usable width without clipping.
                            ui.add_enabled_ui(
                                view.pending_preview.is_none()
                                    && state.confirmation == EditorConfirmation::None,
                                |ui| {
                                    let list_height = (body_height * 0.64).max(110.0);
                                    if ui.available_width() >= 680.0 {
                                        ui.columns(2, |cols| {
                                            source_list(
                                                &mut cols[0],
                                                view,
                                                state,
                                                list_height,
                                                &mut actions,
                                                &mut thumbnails,
                                            );
                                            unit_list(
                                                &mut cols[1],
                                                view,
                                                state,
                                                list_height,
                                                &mut actions,
                                                &mut thumbnails,
                                            );
                                        });
                                    } else {
                                        ui.horizontal(|ui| {
                                            ui.selectable_value(
                                                &mut state.panel,
                                                EditorPanel::Sources,
                                                "元ページ一覧",
                                            );
                                            ui.selectable_value(
                                                &mut state.panel,
                                                EditorPanel::Units,
                                                "編集する見開き",
                                            );
                                        });
                                        match state.panel {
                                            EditorPanel::Sources => source_list(
                                                ui,
                                                view,
                                                state,
                                                list_height,
                                                &mut actions,
                                                &mut thumbnails,
                                            ),
                                            EditorPanel::Units => unit_list(
                                                ui,
                                                view,
                                                state,
                                                list_height,
                                                &mut actions,
                                                &mut thumbnails,
                                            ),
                                        }
                                    }
                                },
                            );
                            ui.separator();
                            ui.label(format!("保存後の元の窓: {}", view.landing));
                        }
                    }
                });
        },
    );
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                editing
                    && view.can_save
                    && view.pending_preview.is_none()
                    && state.confirmation == EditorConfirmation::None,
                egui::Button::new("保存"),
            )
            .clicked()
        {
            actions.push(EditorUiAction::Save);
        }
        if ui
            .add_enabled(
                !matches!(view.phase, EditorPhase::Saving | EditorPhase::Reloading),
                egui::Button::new("キャンセル"),
            )
            .clicked()
        {
            actions.push(EditorUiAction::Cancel);
        }
        if ui
            .add_enabled(
                editing
                    && view.can_remove
                    && view.pending_preview.is_none()
                    && state.confirmation == EditorConfirmation::None,
                egui::Button::new("構成を解除"),
            )
            .clicked()
        {
            state.confirmation = EditorConfirmation::Remove;
        }
    });
    if !thumbnails.is_empty() {
        actions.push(EditorUiAction::RequestThumbnails(thumbnails));
    }
    actions
}

fn settings(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    actions: &mut Vec<EditorUiAction>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("読む方向:");
        for (value, name) in [
            (ReadingDirection::Rtl, "右開き"),
            (ReadingDirection::Ltr, "左開き"),
        ] {
            if ui.selectable_label(view.direction == value, name).clicked() {
                actions.push(EditorUiAction::SetDirection(value));
            }
        }
        ui.label(if view.cover.is_some() {
            "表紙あり"
        } else {
            "表紙なし"
        });
        if ui
            .add_enabled(view.cover.is_some(), egui::Button::new("表紙を解除"))
            .clicked()
        {
            actions.push(EditorUiAction::SetCover(None));
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(view.can_undo, egui::Button::new("元に戻す"))
            .clicked()
        {
            actions.push(EditorUiAction::Undo);
        }
        if ui
            .add_enabled(view.can_redo, egui::Button::new("やり直す"))
            .clicked()
        {
            actions.push(EditorUiAction::Redo);
        }
        if ui.button("編集中の条件で組を作り直す").clicked() {
            state.confirmation = EditorConfirmation::Rebuild;
        }
    });
}

fn confirmation(ui: &mut egui::Ui, state: &mut EditorUiState, actions: &mut Vec<EditorUiAction>) {
    let remove = state.confirmation == EditorConfirmation::Remove;
    let prompt = egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(if remove { "本全体の構成を解除します。ページの非表示と並び順を取り除き、自動規則へ戻します。元ファイルは変更しません。" } else { "編集中のページ順・方向・表紙を使って、隣り合うページの組を作り直します。全体の既定や見開きの自動調整は適用し直しません。変更される組は次の確認画面で表示します。" });
        ui.horizontal_wrapped(|ui| {
            if ui.button(if remove { "解除する" } else { "作り直す" }).clicked() {
                actions.push(if remove { EditorUiAction::Remove } else { EditorUiAction::Rebuild });
                state.confirmation = EditorConfirmation::None;
            }
            if ui.button("編集を続ける").clicked() { state.confirmation = EditorConfirmation::None; }
        });
    });
    ui.scroll_to_rect(prompt.response.rect, Some(egui::Align::Min));
}

fn selected_controls(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &EditorUiState,
    actions: &mut Vec<EditorUiAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if let Some(&page) = state
            .selected_pages
            .first()
            .filter(|_| state.selected_pages.len() == 1)
            && let Some(p) = view.pages.get(page)
        {
            if ui
                .button(if p.hidden {
                    "表示に戻す"
                } else {
                    "非表示"
                })
                .clicked()
            {
                actions.push(EditorUiAction::SetHidden {
                    page,
                    hidden: !p.hidden,
                });
            }
            if ui.button("白紙を挿入").clicked() {
                actions.push(EditorUiAction::InsertBlank {
                    page,
                    placement: SinglePlacement::Leading,
                });
            }
            if ui
                .add_enabled(
                    !p.hidden && !p.missing && !p.excluded_by_policy,
                    egui::Button::new("表紙に指定"),
                )
                .clicked()
            {
                actions.push(EditorUiAction::SetCover(Some(page)));
            }
        }
        if state.selected_pages.len() == 2 {
            if ui.button("この2枚を組にする").clicked() {
                actions.push(EditorUiAction::Pair {
                    first: state.selected_pages[0],
                    second: state.selected_pages[1],
                });
            }
        } else {
            ui.weak("ページを選択。Ctrlを押して2枚選ぶと組にできます。");
        }
    });
    if let Some(unit) = state.selected_unit.filter(|&i| i < view.units.len()) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(unit > 0, egui::Button::new("前へ"))
                .clicked()
            {
                actions.push(EditorUiAction::MoveUnit { unit, to: unit - 1 });
            }
            if ui
                .add_enabled(unit + 1 < view.units.len(), egui::Button::new("後へ"))
                .clicked()
            {
                actions.push(EditorUiAction::MoveUnit { unit, to: unit + 1 });
            }
            match &view.units[unit] {
                EditorUnitView::Pair { .. } => {
                    if ui.button("単ページにする").clicked() {
                        actions.push(EditorUiAction::SplitUnit(unit));
                    }
                }
                EditorUnitView::Single {
                    placement,
                    companion,
                    ..
                } => {
                    for (value, name) in [
                        (SinglePlacement::Center, "中央"),
                        (SinglePlacement::Leading, "読む順の先頭側"),
                        (SinglePlacement::Trailing, "読む順の後ろ側"),
                    ] {
                        if ui.selectable_label(*placement == value, name).clicked() {
                            actions.push(EditorUiAction::SetPlacement {
                                unit,
                                placement: value,
                            });
                        }
                    }
                    if ui
                        .button(if *companion == EditorCompanionView::Blank {
                            "白紙を取り除く"
                        } else {
                            "白紙を挿入"
                        })
                        .clicked()
                    {
                        actions.push(EditorUiAction::SetCompanion {
                            unit,
                            companion: if *companion == EditorCompanionView::Blank {
                                EditorCompanionView::Vacant
                            } else {
                                EditorCompanionView::Blank
                            },
                        });
                    }
                    let is_cover = matches!(companion, EditorCompanionView::Cover(_));
                    if ui
                        .add_enabled(
                            view.cover.is_some() || is_cover,
                            egui::Button::new(if is_cover {
                                "表紙の補助表示を外す"
                            } else {
                                "表紙の補助表示"
                            }),
                        )
                        .clicked()
                    {
                        actions.push(EditorUiAction::SetCompanion {
                            unit,
                            companion: if is_cover {
                                EditorCompanionView::Vacant
                            } else {
                                EditorCompanionView::Cover(view.cover.unwrap())
                            },
                        });
                    }
                }
            }
        });
    }
}

fn select_page(state: &mut EditorUiState, page: usize, additive: bool) {
    state.selected_unit = None;
    if additive {
        if let Some(at) = state.selected_pages.iter().position(|&p| p == page) {
            state.selected_pages.remove(at);
        } else {
            if state.selected_pages.len() == 2 {
                state.selected_pages.remove(0);
            }
            state.selected_pages.push(page);
        }
    } else {
        state.selected_pages = vec![page];
    }
}

fn source_list(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    height: f32,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    ui.label(format!("元ページ一覧 ({}ページ)", view.pages.len()));
    let row = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .id_salt("composition_sources")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, row, view.pages.len(), |ui, range| {
            for index in range {
                let page = &view.pages[index];
                if page.thumbnail.is_none()
                    && !page.excluded_by_policy
                    && !page.missing
                    && thumbnails.len() < 24
                    && !thumbnails.contains(&page.id)
                {
                    thumbnails.push(page.id);
                }
                let suffix = if page.excluded_by_policy {
                    " / ファイル種類の設定で非表示"
                } else if page.missing {
                    " / 見つからないページ"
                } else if page.hidden {
                    " / 非表示"
                } else {
                    ""
                };
                let label = format!("{}{}: {}", page.physical_number, suffix, page.name);
                let button = if let Some((texture, size)) = page
                    .thumbnail
                    .filter(|_| !page.excluded_by_policy && !page.missing)
                {
                    egui::Button::image_and_text(
                        egui::Image::new((texture, size)).fit_to_exact_size(egui::vec2(row, row)),
                        label,
                    )
                } else {
                    egui::Button::new(label)
                };
                let r = ui.add_sized(
                    egui::vec2(ui.available_width(), row),
                    button
                        .truncate()
                        .selected(state.selected_pages.contains(&page.id))
                        .sense(Sense::click_and_drag()),
                );
                if r.clicked() {
                    select_page(state, page.id, ui.input(|i| i.modifiers.ctrl));
                }
                r.dnd_set_drag_payload(DragItem::Page(page.id));
                if let Some(payload) = r.dnd_release_payload::<DragItem>()
                    && let DragItem::Page(from) = *payload
                    && from != page.id
                {
                    actions.push(EditorUiAction::MovePage {
                        page: from,
                        before: page.id,
                    });
                }
                r.on_hover_text(format!("{}{}", page.name, suffix));
            }
        });
    if let Some(&page) = state
        .selected_pages
        .first()
        .filter(|_| state.selected_pages.len() == 1)
    {
        ui.horizontal_wrapped(|ui| {
            let unit = view.units.iter().position(|u| match *u {
                EditorUnitView::Pair { first, second } => page == first || page == second,
                EditorUnitView::Single { page: p, .. } => page == p,
            });
            if ui
                .add_enabled(
                    unit.is_some_and(|u| u > 0),
                    egui::Button::new("ページを前へ"),
                )
                .clicked()
            {
                actions.push(EditorUiAction::MovePageStep {
                    page,
                    forward: false,
                });
            }
            if ui
                .add_enabled(
                    unit.is_some_and(|u| u + 1 < view.units.len()),
                    egui::Button::new("ページを後へ"),
                )
                .clicked()
            {
                actions.push(EditorUiAction::MovePageStep {
                    page,
                    forward: true,
                });
            }
        });
    }
}

fn unit_list(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    height: f32,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    ui.label(format!("編集する見開き ({}組)", view.units.len()));
    let row_height = 110.0;
    egui::ScrollArea::vertical()
        .id_salt("composition_units")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, row_height, view.units.len(), |ui, range| {
            for unit in range {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), row_height),
                    Sense::click_and_drag(),
                );
                ui.painter().rect_filled(
                    rect.shrink(2.0),
                    4.0,
                    if state.selected_unit == Some(unit) {
                        ui.visuals().selection.bg_fill
                    } else {
                        ui.visuals().faint_bg_color
                    },
                );
                if response.clicked() {
                    state.selected_unit = Some(unit);
                    state.selected_pages.clear();
                }
                response.dnd_set_drag_payload(DragItem::Unit(unit));
                if let Some(payload) = response.dnd_release_payload::<DragItem>()
                    && let DragItem::Unit(from) = *payload
                    && from != unit
                {
                    actions.push(EditorUiAction::MoveUnit {
                        unit: from,
                        to: unit,
                    });
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(6.0)), |ui| {
                    ui.set_clip_rect(rect.shrink(2.0).intersect(ui.clip_rect()));
                    let placement = match view.units[unit] {
                        EditorUnitView::Pair { .. } => "見開き",
                        EditorUnitView::Single { placement, .. } => {
                            match placement.screen_slot(view.direction) {
                                ScreenSlot::Center => "中央",
                                ScreenSlot::Left => "左側",
                                ScreenSlot::Right => "右側",
                            }
                        }
                    };
                    ui.label(format!("{}組目 / {}", unit + 1, placement));
                    ui.horizontal(|ui| match view.units[unit] {
                        EditorUnitView::Pair { first, second } => {
                            let (left, right) = if view.direction == ReadingDirection::Ltr {
                                (first, second)
                            } else {
                                (second, first)
                            };
                            page_card(ui, view, state, left, actions, thumbnails);
                            page_card(ui, view, state, right, actions, thumbnails);
                        }
                        EditorUnitView::Single {
                            page,
                            placement,
                            companion,
                        } => {
                            let slot = placement.screen_slot(view.direction);
                            if slot == ScreenSlot::Right {
                                companion_card(ui, view, state, companion, actions, thumbnails);
                            }
                            page_card(ui, view, state, page, actions, thumbnails);
                            if slot != ScreenSlot::Right {
                                companion_card(ui, view, state, companion, actions, thumbnails);
                            }
                        }
                    });
                });
            }
        });
}

fn page_card(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    id: usize,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    let Some(page) = view.pages.get(id) else {
        return;
    };
    let (rect, r) = ui.allocate_exact_size(egui::vec2(76.0, 70.0), Sense::click_and_drag());
    if let Some((texture, size)) = page
        .thumbnail
        .filter(|_| !page.excluded_by_policy && !page.missing)
    {
        let fit =
            size * (rect.width() / size.x.max(1.0)).min((rect.height() - 17.0) / size.y.max(1.0));
        ui.painter().image(
            texture,
            egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, 8.0), fit),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter()
            .rect_filled(rect.shrink(1.0), 3.0, ui.visuals().widgets.inactive.bg_fill);
        if !page.excluded_by_policy
            && !page.missing
            && thumbnails.len() < 24
            && !thumbnails.contains(&id)
        {
            thumbnails.push(id);
        }
    }
    ui.painter().text(
        rect.center_bottom() - egui::vec2(0.0, 2.0),
        egui::Align2::CENTER_BOTTOM,
        if page.missing {
            "見つからないページ".to_owned()
        } else {
            format!("{}ページ", page.physical_number)
        },
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    if state.selected_pages.contains(&id) {
        ui.painter().rect_stroke(
            rect,
            3.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    if r.clicked() {
        select_page(state, id, ui.input(|i| i.modifiers.ctrl));
    }
    r.dnd_set_drag_payload(DragItem::Page(id));
    if let Some(payload) = r.dnd_release_payload::<DragItem>()
        && let DragItem::Page(from) = *payload
        && from != id
    {
        // Page reorder is returned by the owning card list; selection itself is
        // unchanged here. The adapter handles canonical reading-order placement.
        actions.push(EditorUiAction::MovePage {
            page: from,
            before: id,
        });
    }
    r.on_hover_text(format!(
        "{}{}",
        page.name,
        if page.missing {
            " / 見つからないページ"
        } else if page.excluded_by_policy {
            " / ファイル種類の設定で非表示"
        } else {
            ""
        }
    ));
}

fn companion_card(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    companion: EditorCompanionView,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    match companion {
        EditorCompanionView::Vacant => {}
        EditorCompanionView::Cover(id) => {
            // Auxiliary copies do not become selection or drag sources.
            let saved_selection = state.selected_pages.clone();
            ui.add_enabled_ui(false, |ui| {
                page_card(ui, view, state, id, actions, thumbnails)
            });
            state.selected_pages = saved_selection;
            ui.weak("補助");
        }
        EditorCompanionView::Blank => {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(94.0, 52.0), Sense::hover());
            let color = ui.visuals().weak_text_color().gamma_multiply(0.6);
            ui.painter()
                .rect_filled(rect, 3.0, ui.visuals().faint_bg_color);
            let stroke = egui::Stroke::new(1.0, color);
            for (a, b) in [
                (rect.left_top(), rect.right_top()),
                (rect.right_top(), rect.right_bottom()),
                (rect.right_bottom(), rect.left_bottom()),
                (rect.left_bottom(), rect.left_top()),
            ] {
                ui.painter()
                    .add(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 3.0));
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "この見開きの\n空き側",
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}
