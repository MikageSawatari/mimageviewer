//! Pure editor presentation. Only the left list owns order/hidden mutations.
//! Result groups are derived by the owning session worker; this module has no I/O.
use crate::book_composition::{
    PageOverride, PageSide, ReadingDirection, ScreenSlot, SinglePlacement,
};
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
    pub id: usize,
    pub name: String,
    pub physical_number: usize,
    pub hidden: bool,
    pub missing: bool,
    pub excluded_by_policy: bool,
    pub thumbnail: Option<(egui::TextureId, egui::Vec2)>,
    pub directive: Option<PageOverride>,
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
    pub cover_enabled: bool,
    pub cover: Option<usize>,
    pub pages: Vec<EditorPageView>,
    pub page_order: Vec<usize>,
    pub units: Vec<EditorUnitView>,
    pub results_updating: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_save: bool,
    pub can_remove: bool,
    pub landing: String,
    pub error: Option<String>,
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
}
#[derive(Clone, Debug, Default)]
pub struct EditorUiState {
    pub selected_pages: Vec<usize>,
    pub panel: EditorPanel,
    pub confirmation: EditorConfirmation,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct DialogKeys {
    pub enter: bool,
    pub escape: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum EditorUiAction {
    SetDirection(ReadingDirection),
    SetCover(bool),
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
    SetOverride {
        page: usize,
        directive: Option<PageOverride>,
    },
    Undo,
    Redo,
    Reload,
    IgnoreComposition,
    Save,
    Cancel,
    Remove,
    RequestThumbnails(Vec<usize>),
}
#[derive(Clone, Copy)]
struct DragItem(usize);
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

pub fn render(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
) -> Vec<EditorUiAction> {
    let mut actions = Vec::new();
    let mut thumbnails = Vec::new();
    let editing = view.phase == EditorPhase::Editing;
    let narrow = ui.available_width() < 680.0;
    let landing = format!("保存後の元の窓: {}", view.landing);
    // Font style height does not include a taller Japanese fallback glyph.
    // Reserve the actual laid-out text and button padding used by these widgets,
    // rounding each independent row up to a physical pixel before summing.
    let pixels_per_point = ui.pixels_per_point();
    let ceil_pixel = |height: f32| (height * pixels_per_point).ceil() / pixels_per_point;
    let button_font = egui::TextStyle::Button.resolve(ui.style());
    let button_text_sizes: Vec<_> = ["元に戻す", "やり直す", "保存", "キャンセル", "構成を解除"]
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    (*label).to_owned(),
                    button_font.clone(),
                    ui.visuals().text_color(),
                )
                .size()
        })
        .collect();
    let button_text_height = button_text_sizes
        .iter()
        .map(|size| size.y)
        .fold(0.0_f32, f32::max);
    let button_row_width = button_text_sizes
        .iter()
        .map(|size| size.x + 2.0 * ui.spacing().button_padding.x)
        .sum::<f32>()
        + 4.0 * ui.spacing().item_spacing.x;
    let footer_narrow = ui.available_width() < button_row_width;
    let row = ceil_pixel(
        ui.spacing()
            .interact_size
            .y
            .max(button_text_height + 2.0 * ui.spacing().button_padding.y),
    );
    let landing_height = ceil_pixel(
        ui.painter()
            .layout_no_wrap(
                landing.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .y,
    );
    let footer_height = row * if footer_narrow { 2.0 } else { 1.0 }
        + landing_height
        + if footer_narrow { 3.0 } else { 2.0 } * ui.spacing().item_spacing.y
        + 6.0;
    // Allocate the entire bounded surface once. Natural child content must not
    // move the footer, and Window's resizable inner size must not extend the
    // interactive content beyond the current screen.
    let available = ui
        .available_rect_before_wrap()
        .intersect(ui.ctx().content_rect().shrink(12.0));
    let (surface, _) = ui.allocate_exact_size(available.size(), Sense::hover());
    let footer_top = (((surface.bottom() - footer_height) * pixels_per_point).floor()
        / pixels_per_point)
        .max(surface.top());
    let body_rect = egui::Rect::from_min_max(surface.min, egui::pos2(surface.right(), footer_top));
    let footer_rect = egui::Rect::from_min_max(egui::pos2(surface.left(), footer_top), surface.max);
    let mut body = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("editor_body")
            .max_rect(body_rect),
    );
    body.set_clip_rect(ui.clip_rect().intersect(body_rect));
    {
        let ui = &mut body;
        ui.horizontal(|ui| {
            if editing && let Some(error) = &view.error {
                if ui
                    .button(RichText::new("ページを読み直す").color(ui.visuals().error_fg_color))
                    .on_hover_text(error)
                    .clicked()
                {
                    actions.push(EditorUiAction::Reload);
                }
            }
            ui.add(egui::Label::new(RichText::new(&view.book_name).strong()).truncate())
                .on_hover_text(&view.book_name);
        });
        if editing {
            ui.horizontal_wrapped(|ui| {
                ui.weak(if view.saved {
                    "保存済みの構成"
                } else {
                    "新しい構成"
                })
                .on_hover_text(
                    "ページ順と指定から自動生成します。見開きずらしは構成に保存しません",
                );
                for (direction, name) in [
                    (ReadingDirection::Rtl, "右開き"),
                    (ReadingDirection::Ltr, "左開き"),
                ] {
                    if ui
                        .selectable_label(view.direction == direction, name)
                        .clicked()
                    {
                        actions.push(EditorUiAction::SetDirection(direction));
                    }
                }
                let mut cover = view.cover_enabled;
                if ui.checkbox(&mut cover, "表紙あり").changed() {
                    actions.push(EditorUiAction::SetCover(cover));
                }
            });
            if state.confirmation == EditorConfirmation::Remove {
                ui.add(egui::Label::new("保存済みの順序・非表示・指定を解除しますか").truncate());
                ui.horizontal(|ui| {
                    if ui.button("解除する").clicked() {
                        actions.push(EditorUiAction::Remove);
                        state.confirmation = EditorConfirmation::None;
                    }
                    if ui.button("編集を続ける").clicked() {
                        state.confirmation = EditorConfirmation::None;
                    }
                });
            }
            ui.add_enabled_ui(state.confirmation == EditorConfirmation::None, |ui| {
                if !narrow {
                    let height = ui.available_height().max(0.0);
                    ui.columns(2, |columns| {
                        source_list(
                            &mut columns[0],
                            view,
                            state,
                            height,
                            true,
                            &mut actions,
                            &mut thumbnails,
                        );
                        unit_list(
                            &mut columns[1],
                            view,
                            state,
                            height,
                            true,
                            &mut actions,
                            &mut thumbnails,
                        );
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut state.panel,
                            EditorPanel::Sources,
                            format!("元ページ一覧 ({}ページ)", view.pages.len()),
                        );
                        ui.selectable_value(
                            &mut state.panel,
                            EditorPanel::Units,
                            format!(
                                "自動で組んだ結果 ({}組){}",
                                view.units.len(),
                                if view.results_updating {
                                    " / 更新中"
                                } else {
                                    ""
                                }
                            ),
                        )
                        .on_hover_text(
                            "白紙は見開きだけに使い、単ページ・横長分割では表示しません",
                        );
                    });
                    let height = ui.available_height().max(0.0);
                    match state.panel {
                        EditorPanel::Sources => source_list(
                            ui,
                            view,
                            state,
                            height,
                            false,
                            &mut actions,
                            &mut thumbnails,
                        ),
                        EditorPanel::Units => {
                            unit_list(
                                ui,
                                view,
                                state,
                                height,
                                false,
                                &mut actions,
                                &mut thumbnails,
                            );
                        }
                    }
                }
            });
        } else {
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.floating_allocated_width = scroll.bar_inner_margin + scroll.bar_width;
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .max_height(ui.available_height().max(0.0))
                .show(ui, |ui| {
                    if let Some(error) = &view.error {
                        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
                    }
                    ui.label(match view.phase {
                        EditorPhase::Preparing => "本のページを準備しています",
                        EditorPhase::Saving => "構成を保存しています",
                        EditorPhase::Reloading => "保存した構成で本を読み直しています",
                        _ => "構成を読み取れません。保存データを保持しています。",
                    });
                    if view.phase == EditorPhase::Blocked {
                        if ui.button("読み直す").clicked() {
                            actions.push(EditorUiAction::Reload);
                        }
                        if ui.button("今回は構成を使わず開く").clicked() {
                            actions.push(EditorUiAction::IgnoreComposition);
                        }
                    }
                });
        }
    }
    let mut footer = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("editor_footer")
            .max_rect(footer_rect),
    );
    footer.set_clip_rect(ui.clip_rect().intersect(footer_rect));
    let ui = &mut footer;
    ui.separator();
    ui.add(egui::Label::new(landing).truncate());
    let undo_redo = |ui: &mut egui::Ui, actions: &mut Vec<EditorUiAction>| {
        if ui
            .add_enabled(editing && view.can_undo, egui::Button::new("元に戻す"))
            .clicked()
        {
            actions.push(EditorUiAction::Undo);
        }
        if ui
            .add_enabled(editing && view.can_redo, egui::Button::new("やり直す"))
            .clicked()
        {
            actions.push(EditorUiAction::Redo);
        }
    };
    if footer_narrow {
        ui.horizontal(|ui| undo_redo(ui, &mut actions));
    }
    ui.horizontal(|ui| {
        if !footer_narrow {
            undo_redo(ui, &mut actions);
        }
        if ui
            .add_enabled(
                editing && view.can_save && state.confirmation == EditorConfirmation::None,
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
            .add_enabled(editing && view.can_remove, egui::Button::new("構成を解除"))
            .clicked()
        {
            state.confirmation = EditorConfirmation::Remove;
        }
    });
    if !thumbnails.is_empty() {
        actions.push(EditorUiAction::RequestThumbnails(thumbnails));
    }
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::new("composition_editor_geometry"),
            EditorGeometry {
                surface,
                body: body_rect,
                footer: footer_rect,
                actual_footer: ui.min_rect(),
                footer_clip: ui.clip_rect(),
            },
        )
    });
    actions
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct EditorGeometry {
    surface: egui::Rect,
    body: egui::Rect,
    footer: egui::Rect,
    actual_footer: egui::Rect,
    footer_clip: egui::Rect,
}
fn select_page(state: &mut EditorUiState, page: usize) {
    state.selected_pages = vec![page];
}
fn source_list(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    height: f32,
    show_heading: bool,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    if show_heading {
        ui.label(format!("元ページ一覧 ({}ページ)", view.pages.len()));
    }
    ui.horizontal(|ui| {
        let selected = state
            .selected_pages
            .first()
            .and_then(|&id| view.pages.get(id));
        let position = selected.and_then(|p| view.page_order.iter().position(|&id| id == p.id));
        for (forward, name, enabled) in [
            (false, "ページを前へ", position.is_some_and(|p| p > 0)),
            (
                true,
                "ページを後へ",
                position.is_some_and(|p| p + 1 < view.page_order.len()),
            ),
        ] {
            if ui.add_enabled(enabled, egui::Button::new(name)).clicked() {
                actions.push(EditorUiAction::MovePageStep {
                    page: selected.unwrap().id,
                    forward,
                });
            }
        }
        if ui
            .add_enabled(
                selected.is_some(),
                egui::Button::new(if selected.is_some_and(|p| p.hidden) {
                    "表示に戻す"
                } else {
                    "非表示"
                }),
            )
            .clicked()
        {
            let page = selected.unwrap();
            actions.push(EditorUiAction::SetHidden {
                page: page.id,
                hidden: !page.hidden,
            });
        }
    });
    let row =
        ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
        );
    let height = ui.available_height().min(height).max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("composition_sources")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, row, view.page_order.len(), |ui, range| {
            for index in range {
                let page = &view.pages[view.page_order[index]];
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
                    let thumb = (row - 2.0 * ui.spacing().button_padding.y).max(1.0);
                    egui::Button::image_and_text(
                        egui::Image::new((texture, size))
                            .fit_to_exact_size(egui::vec2(thumb, thumb)),
                        label,
                    )
                } else {
                    request_thumbnail(page, thumbnails);
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
                    select_page(state, page.id);
                }
                r.dnd_set_drag_payload(DragItem(page.id));
                if let Some(payload) = r.dnd_release_payload::<DragItem>()
                    && payload.0 != page.id
                {
                    actions.push(EditorUiAction::MovePage {
                        page: payload.0,
                        before: page.id,
                    });
                }
                r.on_hover_text(format!("{}{}", page.name, suffix));
            }
        });
}
fn request_thumbnail(page: &EditorPageView, thumbnails: &mut Vec<usize>) {
    if page.thumbnail.is_none()
        && !page.excluded_by_policy
        && !page.missing
        && thumbnails.len() < 24
        && !thumbnails.contains(&page.id)
    {
        thumbnails.push(page.id);
    }
}
fn directive_controls(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &EditorUiState,
    actions: &mut Vec<EditorUiAction>,
) {
    let selected = state
        .selected_pages
        .first()
        .and_then(|&id| view.pages.get(id))
        .filter(|p| !p.hidden && !p.missing && !p.excluded_by_policy);
    ui.add_enabled_ui(selected.is_some(), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (directive, name) in [
                (None, "自動"),
                (Some(PageOverride::Alone), "単独で表示"),
                (
                    Some(PageOverride::BlankOpposite {
                        page_side: PageSide::Leading,
                    }),
                    "隣を白紙にする",
                ),
            ] {
                let chosen = selected.is_some_and(|p| {
                    if matches!(directive, Some(PageOverride::BlankOpposite { .. })) {
                        matches!(p.directive, Some(PageOverride::BlankOpposite { .. }))
                    } else {
                        p.directive == directive
                    }
                });
                if ui
                    .selectable_label(chosen, name)
                    .on_hover_text("白紙は見開きだけに使い、単ページ・横長分割では表示しません")
                    .clicked()
                {
                    actions.push(EditorUiAction::SetOverride {
                        page: selected.unwrap().id,
                        directive,
                    });
                }
            }
            if let Some(p) = selected
                && let Some(PageOverride::BlankOpposite { page_side }) = p.directive
            {
                ui.weak("実ページ側:");
                for (left, name) in [(true, "左"), (false, "右")] {
                    let side = if left == (view.direction == ReadingDirection::Ltr) {
                        PageSide::Leading
                    } else {
                        PageSide::Trailing
                    };
                    if ui
                        .selectable_label(page_side == side, name)
                        .on_hover_text(if left {
                            "実ページを左に置く"
                        } else {
                            "実ページを右に置く"
                        })
                        .clicked()
                    {
                        actions.push(EditorUiAction::SetOverride {
                            page: p.id,
                            directive: Some(PageOverride::BlankOpposite { page_side: side }),
                        });
                    }
                }
            }
        });
    });
}
fn badge(view: &EditorView, id: usize) -> String {
    let page = &view.pages[id];
    let specified = match page.directive {
        None => "",
        Some(PageOverride::Alone) => "単独",
        Some(PageOverride::BlankOpposite { page_side }) => match (page_side, view.direction) {
            (PageSide::Leading, ReadingDirection::Ltr)
            | (PageSide::Trailing, ReadingDirection::Rtl) => "白紙・左側",
            _ => "白紙・右側",
        },
    };
    if view.cover == Some(id) {
        format!("表紙 {specified}")
    } else {
        specified.into()
    }
}
fn result_row_height(ui: &egui::Ui) -> f32 {
    let label_height = ui
        .painter()
        .layout_no_wrap("0ページ".into(), page_card_label_font(), Color32::WHITE)
        .size()
        .y;
    let description_height = ui
        .painter()
        .layout_no_wrap(
            "0組目 / 白紙・右側".into(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .y;
    // The tallest actual content owns the row. Vertical separation belongs to
    // show_rows' item_spacing; an inset here would consume the minimum viewport
    // needed to show one entire thumbnail and its Japanese label band.
    page_card_height(label_height).max(2.0 * description_height + ui.spacing().item_spacing.y)
}
fn unit_list(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    height: f32,
    show_heading: bool,
    actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) -> egui::scroll_area::ScrollAreaOutput<Vec<(usize, egui::Rect)>> {
    if show_heading {
        ui.label(format!(
            "自動で組んだ結果 ({}組){}",
            view.units.len(),
            if view.results_updating {
                " / 更新中"
            } else {
                ""
            }
        ))
        .on_hover_text("白紙は見開きだけに使い、単ページ・横長分割では表示しません");
    }
    directive_controls(ui, view, state, actions);
    let row_height = result_row_height(ui);
    let height = ui.available_height().min(height).max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("composition_units")
        .max_height(height)
        .auto_shrink([false, false])
        .show_rows(ui, row_height, view.units.len(), |ui, range| {
            let mut rectangles = Vec::new();
            for unit in range {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), row_height),
                    Sense::hover(),
                );
                let content_rect = rect.shrink2(egui::vec2(2.0, 0.0));
                ui.painter()
                    .rect_filled(content_rect, 4.0, ui.visuals().faint_bg_color);
                // new_child does not advance the parent's cursor a second time. The
                // only parent allocation is the exact row used by show_rows above.
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("result", unit))
                        .max_rect(content_rect),
                );
                child.set_clip_rect(rect.intersect(ui.clip_rect()));
                if view.results_updating || !ui.is_enabled() {
                    child.disable();
                }
                let (ids, description) = match view.units[unit] {
                    EditorUnitView::Pair { first, second } => (vec![first, second], "見開き"),
                    EditorUnitView::Single {
                        page,
                        companion: EditorCompanionView::Cover(_),
                        ..
                    } => (vec![page], "末尾に添えた表紙"),
                    EditorUnitView::Single {
                        page,
                        companion: EditorCompanionView::Blank,
                        ..
                    } => (
                        vec![page],
                        if matches!(
                            view.pages[page].directive,
                            Some(PageOverride::BlankOpposite { .. })
                        ) {
                            "指定した白紙（末尾表紙より優先）"
                        } else {
                            "自動の単独・白紙"
                        },
                    ),
                    EditorUnitView::Single { page, .. } => (
                        vec![page],
                        if view.pages[page].directive == Some(PageOverride::Alone) {
                            "単独指定"
                        } else {
                            "自動の単独（横長・端など）"
                        },
                    ),
                };
                child.horizontal(|ui| {
                    match view.units[unit] {
                        EditorUnitView::Pair { first, second } => {
                            let (left, right) = if view.direction == ReadingDirection::Ltr {
                                (first, second)
                            } else {
                                (second, first)
                            };
                            page_card(ui, view, state, left, false, actions, thumbnails);
                            page_card(ui, view, state, right, false, actions, thumbnails);
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
                            page_card(ui, view, state, page, false, actions, thumbnails);
                            if slot != ScreenSlot::Right {
                                companion_card(ui, view, state, companion, actions, thumbnails);
                            }
                        }
                    };

                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(format!("{}組目 / {description}", unit + 1))
                                .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                ids.into_iter()
                                    .map(|id| badge(view, id))
                                    .filter(|s| !s.is_empty())
                                    .collect::<Vec<_>>()
                                    .join(" / "),
                            )
                            .truncate(),
                        );
                    });
                });
                rectangles.push((unit, rect.union(child.min_rect())));
            }
            rectangles
        })
}
const PAGE_CARD_WIDTH: f32 = 76.0;
const PAGE_CARD_THUMB_HEIGHT: f32 = 52.0;
const PAGE_CARD_LABEL_GAP: f32 = 3.0;

fn page_card_label_font() -> egui::FontId {
    egui::FontId::proportional(12.0)
}

fn page_card_height(label_height: f32) -> f32 {
    PAGE_CARD_THUMB_HEIGHT + PAGE_CARD_LABEL_GAP + label_height
}

fn page_card(
    ui: &mut egui::Ui,
    view: &EditorView,
    state: &mut EditorUiState,
    id: usize,
    supplement: bool,
    _actions: &mut Vec<EditorUiAction>,
    thumbnails: &mut Vec<usize>,
) {
    let Some(page) = view.pages.get(id) else {
        return;
    };
    // The label gets its own band under the thumbnail, sized from the laid-out
    // text. A fixed card height let the label overlap the image with larger fonts.
    let label = ui.painter().layout_no_wrap(
        if page.missing {
            "見つからないページ".to_owned()
        } else {
            format!("{}ページ", page.physical_number)
        },
        page_card_label_font(),
        ui.visuals().text_color(),
    );
    let width = PAGE_CARD_WIDTH.max(label.size().x);
    let (rect, r) = ui.allocate_exact_size(
        egui::vec2(width, page_card_height(label.size().y)),
        Sense::click(),
    );
    r.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            if supplement {
                format!("末尾に添えた表紙: {}ページ", page.physical_number)
            } else {
                format!("{}ページ", page.physical_number)
            },
        )
    });
    let thumb_rect = egui::Rect::from_min_size(rect.min, egui::vec2(width, PAGE_CARD_THUMB_HEIGHT));
    if let Some((texture, size)) = page
        .thumbnail
        .filter(|_| !page.excluded_by_policy && !page.missing)
    {
        let fit = size
            * (thumb_rect.width() / size.x.max(1.0)).min(thumb_rect.height() / size.y.max(1.0));
        ui.painter().image(
            texture,
            egui::Rect::from_center_size(thumb_rect.center(), fit),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        ui.painter().rect_filled(
            thumb_rect.shrink(1.0),
            3.0,
            ui.visuals().widgets.inactive.bg_fill,
        );
        if !page.excluded_by_policy
            && !page.missing
            && thumbnails.len() < 24
            && !thumbnails.contains(&id)
        {
            thumbnails.push(id);
        }
    }
    let label_pos = egui::pos2(
        rect.center().x - label.size().x / 2.0,
        thumb_rect.bottom() + PAGE_CARD_LABEL_GAP,
    );
    ui.painter()
        .galley(label_pos, label, ui.visuals().text_color());
    if state.selected_pages.contains(&id) {
        ui.painter().rect_stroke(
            rect,
            3.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    if r.clicked() {
        select_page(state, id);
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
                page_card(ui, view, state, id, true, actions, thumbnails)
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

#[cfg(test)]
#[path = "../tests/support/scroll_layout.rs"]
mod scroll_layout;

#[cfg(test)]
mod row_tests {
    use super::scroll_layout::{HeightExpectation, ScrollLayout};
    use super::*;

    fn fixture(count: usize) -> EditorView {
        EditorView {
            book_name: "row geometry".into(),
            saved: true,
            phase: EditorPhase::Editing,
            direction: ReadingDirection::Rtl,
            cover_enabled: true,
            cover: Some(0),
            pages: (0..count)
                .map(|id| EditorPageView {
                    id,
                    name: format!("page-{id}"),
                    physical_number: id + 1,
                    hidden: false,
                    missing: false,
                    excluded_by_policy: false,
                    thumbnail: None,
                    directive: Some(PageOverride::BlankOpposite {
                        page_side: PageSide::Leading,
                    }),
                })
                .collect(),
            page_order: (0..count).collect(),
            units: (0..count)
                .map(|page| EditorUnitView::Single {
                    page,
                    placement: SinglePlacement::Leading,
                    companion: EditorCompanionView::Blank,
                })
                .collect(),
            results_updating: false,
            can_undo: true,
            can_redo: true,
            can_save: true,
            can_remove: true,
            landing: String::new(),
            error: None,
        }
    }

    #[test]
    fn fixed_footer_is_contained_when_body_natural_height_overflows() {
        for (width, height) in [(1093.0, 614.0), (1366.0, 728.0)] {
            for scale in [1.0, 2.0] {
                for phase in [
                    EditorPhase::Preparing,
                    EditorPhase::Editing,
                    EditorPhase::Saving,
                    EditorPhase::Reloading,
                    EditorPhase::Blocked,
                ] {
                    for confirmation in [EditorConfirmation::None, EditorConfirmation::Remove] {
                        let ctx = egui::Context::default();
                        ctx.set_pixels_per_point(scale);
                        crate::ui_fonts::configure_fonts(&ctx);
                        let mut view = fixture(600);
                        view.phase = phase;
                        view.error = Some(
                            "長いエラー文とパスを含む本文が操作行を押し出さないことを確認します。"
                                .repeat(80),
                        );
                        let mut state = EditorUiState {
                            confirmation,
                            selected_pages: vec![1],
                            ..Default::default()
                        };
                        let screen = egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width / scale, height / scale),
                        );
                        for _ in 0..5 {
                            let _ = ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(screen),
                                    ..Default::default()
                                },
                                |ctx| {
                                    show_window(
                                        ctx,
                                        egui::Id::new("footer_geometry_window"),
                                        &view,
                                        &mut state,
                                        DialogKeys::default(),
                                    );
                                },
                            );
                        }
                        let geometry = ctx
                            .data(|data| {
                                data.get_temp::<EditorGeometry>(egui::Id::new(
                                    "composition_editor_geometry",
                                ))
                            })
                            .unwrap();
                        assert!(
                            screen.contains_rect(geometry.surface),
                            "surface escapes screen: {phase:?}/{confirmation:?} scale={scale}"
                        );
                        assert!(
                            geometry.footer.contains_rect(geometry.actual_footer),
                            "natural footer escapes its budget: {phase:?}/{confirmation:?} scale={scale}: {:?} vs {:?}",
                            geometry.actual_footer,
                            geometry.footer
                        );
                        assert!(
                            geometry.footer_clip.contains_rect(geometry.actual_footer),
                            "footer interactions clipped: {phase:?}/{confirmation:?} scale={scale}"
                        );
                        assert!(
                            geometry.body.bottom() <= geometry.footer.top(),
                            "body and footer overlap"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn short_result_list_fits_without_unnecessary_scroll() {
        let ctx = egui::Context::default();
        let view = fixture(2);
        let mut state = EditorUiState::default();
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(620.0, 640.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let output =
                            unit_list(ui, &view, &mut state, 540.0, true, &mut vec![], &mut vec![]);
                        ScrollLayout::from_output(&output).assert_height(
                            HeightExpectation::Fits,
                            ctx.content_rect(),
                            ctx.pixels_per_point(),
                        );
                        assert_eq!(output.inner.len(), 2);
                        assert!(output.inner[0].1.bottom() <= output.inner[1].1.top());
                    });
                },
            );
        }
    }

    #[test]
    fn result_rows_have_one_allocation_and_do_not_overlap_across_virtual_transitions() {
        for scale in [1.0, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            ctx.style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
            let view = fixture(100);
            let mut state = EditorUiState::default();
            let mut previous_layout = None;
            let mut origin: Option<f32> = None;
            let mut first_indices = Vec::new();
            for frame in 0..12 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(620.0, 420.0),
                        )),
                        events: if frame >= 4 {
                            vec![
                                egui::Event::PointerMoved(egui::pos2(300.0, 250.0)),
                                egui::Event::MouseWheel {
                                    unit: egui::MouseWheelUnit::Point,
                                    delta: egui::vec2(0.0, -800.0),
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.spacing_mut().item_spacing.y = 13.0;
                            let pitch = result_row_height(ui) + ui.spacing().item_spacing.y;
                            // egui draws from begin()'s loaded offset, and only
                            // applies this frame's wheel in end(). Output.state
                            // therefore describes the next render's offset.
                            let rendered_offset = egui::scroll_area::State::load(ctx, ui.make_persistent_id(egui::Id::new("composition_units")))
                                .unwrap_or_default().offset.y;
                            let output =
                                unit_list(ui, &view, &mut state, 360.0, true, &mut vec![], &mut vec![]);
                            let layout = ScrollLayout::from_output(&output);
                            layout.assert_height(
                                HeightExpectation::Scrolls,
                                ctx.content_rect(),
                                ctx.pixels_per_point(),
                            );
                            for pair in output.inner.windows(2) {
                                assert!(
                                    pair[0].1.bottom() <= pair[1].1.top(),
                                    "adjacent actual content overlaps: {pair:?}"
                                );
                                assert!(
                                    (pair[1].1.top() - pair[0].1.top() - pitch).abs() < 1.0 / scale,
                                    "actual pitch differs from virtual row pitch"
                                );
                            }
                            if let Some(&(index, rect)) = output.inner.first() {
                                first_indices.push(index);
                                let current_origin =
                                    rect.top() + rendered_offset - index as f32 * pitch;
                                assert!((current_origin - layout.inner.top()).abs() <= 1.0 / scale,
                                    "drawn rows do not match the actual viewport origin: {current_origin} vs {:?}", layout.inner);
                                if frame >= 3 {
                                    if let Some(origin) = origin {
                                        assert!(
                                            (current_origin - origin).abs() <= 1.0 / scale,
                                            "virtual row transition shifted content"
                                        );
                                    } else {
                                        origin = Some(current_origin);
                                    }
                                }
                            }
                            if frame == 3 {
                                previous_layout = Some(layout);
                            }
                            if frame == 11 {
                                assert!(layout.offset.y > previous_layout.unwrap().offset.y);
                            }
                        });
                    },
                );
            }
            assert!(
                first_indices.iter().any(|&index| index > 0),
                "wheel never crossed a virtual row boundary"
            );
        }
    }
}
