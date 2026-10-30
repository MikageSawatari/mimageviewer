//! See [`ScrollArea`] for docs.

#![expect(clippy::needless_range_loop)]

use core::ops::{Add, AddAssign, BitOr, BitOrAssign};

use emath::GuiRounding as _;
use epaint::{Color32, Direction, Margin, Shape};

use crate::{
    AsIdSalt, Context, CursorIcon, Id, IdSalt, MouseWheelSource, NumExt as _, Pos2, Rangef, Rect,
    Response, Role, Sense, Ui, UiBuilder, UiKind, UiStackInfo, Vec2, Vec2b, WidgetInfo, emath,
    epaint, lerp, pass_state, pos2, remap, remap_clamp, style::KineticScrollStyle,
};

use super::scroll_physics::{AxisPhysics, WheelScroll, scroll_bounds};

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
struct ScrollingToTarget {
    animation_time_span: (f64, f64),
    target_offset: f32,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct State {
    /// Positive offset means scrolling down/right.
    ///
    /// This can be outside of the valid range while the user is
    /// rubber-banding past the edge of the content (see [`crate::style::KineticScrollStyle::rubber_band`]).
    /// See [`Self::clamped_offset`] and [`Self::unclamped_offset`].
    offset: Vec2,

    /// The largest valid offset, as of last frame: `content_size - viewport_size`.
    ///
    /// Can be negative if the content is smaller than the viewport.
    max_offset: Vec2,

    /// If set, quickly but smoothly scroll to this target offset.
    offset_target: [Option<ScrollingToTarget>; 2],

    /// Were the scroll bars visible last frame?
    show_scroll: Vec2b,

    /// The content were to large to fit large frame.
    content_is_too_large: Vec2b,

    /// Did the user interact (hover or drag) the scroll bars last frame?
    scroll_bar_interaction: Vec2b,

    /// Velocity of the offset, used for kinetic scrolling and rubber-banding.
    ///
    /// Positive means the offset is increasing, i.e. the content is scrolling down/right.
    #[cfg_attr(feature = "serde", serde(skip))]
    vel: Vec2,

    /// Mouse offset relative to the top of the handle when started moving the handle.
    scroll_start_offset_from_top_left: [Option<f32>; 2],

    /// Is the scroll sticky. This is true while scroll handle is in the end position
    /// and remains that way until the user moves the `scroll_handle`. Once unstuck (false)
    /// it remains false until the scroll touches the end position, which reenables stickiness.
    scroll_stuck_to_end: Vec2b,

    /// Area that can be dragged. This is the size of the content from the last frame.
    interact_rect: Option<Rect>,

    /// Did we already bounce off the edge during the current momentum scroll?
    ///
    /// On macOS, after a quick trackpad swipe the OS keeps sending scroll events
    /// long after the fingers have left the trackpad, with decaying speed.
    /// That is how kinetic scrolling works there (see [`crate::MouseWheelSource::Momentum`]).
    /// When those reach the edge we bounce once, and then ignore the rest of them,
    /// so they don't keep pushing the content out.
    #[cfg_attr(feature = "serde", serde(skip))]
    bounced_this_momentum: Vec2b,

    /// Runtime association only; saved geometry does not restore input authority.
    #[cfg_attr(feature = "serde", serde(skip))]
    association: ScrollAssociation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScrollAssociation {
    #[default]
    Native,
    ExternalVertical(Id),
}

impl Default for State {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            max_offset: Vec2::ZERO,
            offset_target: Default::default(),
            show_scroll: Vec2b::FALSE,
            content_is_too_large: Vec2b::FALSE,
            scroll_bar_interaction: Vec2b::FALSE,
            vel: Vec2::ZERO,
            scroll_start_offset_from_top_left: [None; 2],
            scroll_stuck_to_end: Vec2b::TRUE,
            interact_rect: None,
            bounced_this_momentum: Vec2b::FALSE,
            association: ScrollAssociation::Native,
        }
    }
}

impl State {
    pub fn load(ctx: &Context, id: Id) -> Option<Self> {
        ctx.data_mut(|d| d.get_persisted(id))
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| d.insert_persisted(id, self));
    }

    /// The current kinetic scrolling velocity of the offset, in ui points per second.
    ///
    /// Positive means the offset is increasing, i.e. the content is scrolling down/right.
    pub fn velocity(&self) -> Vec2 {
        self.vel
    }

    /// The scroll offset, clamped to the valid range `0..=max_offset`.
    ///
    /// Positive means scrolled down/right.
    ///
    /// This differs from [`Self::unclamped_offset`] only while the user is rubber-banding
    /// past the edge of the content.
    pub fn clamped_offset(&self) -> Vec2 {
        Vec2::new(
            scroll_bounds(self.max_offset.x).clamp(self.offset.x),
            scroll_bounds(self.max_offset.y).clamp(self.offset.y),
        )
    }

    /// The scroll offset, which can be outside of the valid range while the user is
    /// rubber-banding past the edge of the content
    /// (see [`crate::style::KineticScrollStyle::rubber_band`]).
    ///
    /// Positive means scrolled down/right.
    /// This is where the content is actually drawn.
    pub fn unclamped_offset(&self) -> Vec2 {
        self.offset
    }

    /// How far past the edge of the content we are, in ui points.
    ///
    /// Negative means past the start (top/left), positive means past the end (bottom/right).
    /// Zero when not rubber-banding.
    pub fn overscroll(&self) -> Vec2 {
        self.offset - self.clamped_offset()
    }

    #[deprecated = "Use `clamped_offset()` or `unclamped_offset()`"]
    pub fn offset(&self) -> Vec2 {
        self.clamped_offset()
    }
}

/// Is `ui` inside another [`ScrollArea`]?
fn has_scrolling_ancestor(ui: &Ui) -> bool {
    ui.stack()
        .iter()
        .any(|stack| stack.kind() == Some(UiKind::ScrollArea))
}

pub struct ScrollAreaOutput<R> {
    /// What the user closure returned.
    pub inner: R,

    /// [`Id`] of the [`ScrollArea`].
    pub id: Id,

    /// The current state of the scroll area.
    pub state: State,

    /// The size of the content. If this is larger than [`Self::inner_rect`],
    /// then there was need for scrolling.
    pub content_size: Vec2,

    /// Where on the screen the content is (excludes scroll bars).
    pub inner_rect: Rect,
}

/// Indicate whether the horizontal and vertical scroll bars must be always visible, hidden or visible when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ScrollBarVisibility {
    /// Hide scroll bar even if they are needed.
    ///
    /// You can still scroll, with the scroll-wheel
    /// and by dragging the contents, but there is no
    /// visual indication of how far you have scrolled.
    AlwaysHidden,

    /// Show scroll bars only when the content size exceeds the container,
    /// i.e. when there is any need to scroll.
    ///
    /// This is the default.
    VisibleWhenNeeded,

    /// Always show the scroll bar, even if the contents fit in the container
    /// and there is no need to scroll.
    AlwaysVisible,
}

impl Default for ScrollBarVisibility {
    #[inline]
    fn default() -> Self {
        Self::VisibleWhenNeeded
    }
}

impl ScrollBarVisibility {
    pub const ALL: [Self; 3] = [
        Self::AlwaysHidden,
        Self::VisibleWhenNeeded,
        Self::AlwaysVisible,
    ];
}

/// When [`ScrollArea`] should let the user scroll by dragging the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum DragScroll {
    /// Never scroll on pointer drag.
    Never,

    /// Only allow drag-to-scroll when a touch screen is detected
    /// (see [`crate::InputState::has_touch_screen`]). The recommended default.
    #[default]
    OnTouch,

    /// Always allow drag-to-scroll, even with a mouse.
    Always,
}

impl DragScroll {
    /// Whether drag-to-scroll is currently active.
    ///
    /// Checks if we have a touch screen (via [`crate::InputState::has_touch_screen`])
    /// when `self` is [`Self::OnTouch`].
    pub fn enabled(self, ctx: &Context) -> bool {
        match self {
            Self::Never => false,
            Self::OnTouch => ctx.input(|i| i.has_touch_screen()),
            Self::Always => true,
        }
    }
}

impl BitOr for DragScroll {
    type Output = Self;

    /// Combine two settings, picking the more permissive one.
    /// `Always > OnTouch > Never`.
    #[inline]
    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Always, _) | (_, Self::Always) => Self::Always,
            (Self::OnTouch, _) | (_, Self::OnTouch) => Self::OnTouch,
            (Self::Never, Self::Never) => Self::Never,
        }
    }
}

/// What is the source of scrolling for a [`ScrollArea`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ScrollSource {
    /// Scroll the area by dragging a scroll bar.
    ///
    /// By default the scroll bars remain visible to show current position.
    /// To hide them use [`ScrollArea::scroll_bar_visibility()`].
    pub scroll_bar: bool,

    /// Scroll the area by dragging the contents.
    ///
    /// Defaults to [`DragScroll::OnTouch`]: only active when a touch screen is
    /// detected. Set to [`DragScroll::Always`] to force it on, or
    /// [`DragScroll::Never`] to disable.
    pub drag: DragScroll,

    /// Scroll the area by scrolling (or shift scrolling) the mouse wheel with
    /// the mouse cursor over the [`ScrollArea`].
    pub mouse_wheel: bool,
}

impl Default for ScrollSource {
    /// `scroll_bar` and `mouse_wheel` enabled; `drag` set to [`DragScroll::OnTouch`].
    fn default() -> Self {
        Self {
            scroll_bar: true,
            drag: DragScroll::OnTouch,
            mouse_wheel: true,
        }
    }
}

impl ScrollSource {
    pub const NONE: Self = Self {
        scroll_bar: false,
        drag: DragScroll::Never,
        mouse_wheel: false,
    };
    pub const ALL: Self = Self {
        scroll_bar: true,
        drag: DragScroll::Always,
        mouse_wheel: true,
    };
    pub const SCROLL_BAR: Self = Self {
        scroll_bar: true,
        drag: DragScroll::Never,
        mouse_wheel: false,
    };
    pub const DRAG: Self = Self {
        scroll_bar: false,
        drag: DragScroll::Always,
        mouse_wheel: false,
    };
    pub const MOUSE_WHEEL: Self = Self {
        scroll_bar: false,
        drag: DragScroll::Never,
        mouse_wheel: true,
    };

    /// Is everything disabled?
    #[inline]
    pub fn is_none(&self) -> bool {
        self == &Self::NONE
    }

    /// Is anything enabled?
    #[inline]
    pub fn any(&self) -> bool {
        self.scroll_bar || self.drag != DragScroll::Never || self.mouse_wheel
    }

    /// Is everything enabled?
    #[inline]
    pub fn is_all(&self) -> bool {
        self.scroll_bar && self.drag == DragScroll::Always && self.mouse_wheel
    }
}

impl BitOr for ScrollSource {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            scroll_bar: self.scroll_bar | rhs.scroll_bar,
            drag: self.drag | rhs.drag,
            mouse_wheel: self.mouse_wheel | rhs.mouse_wheel,
        }
    }
}

#[expect(clippy::suspicious_arithmetic_impl)]
impl Add for ScrollSource {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        self | rhs
    }
}

impl BitOrAssign for ScrollSource {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

impl AddAssign for ScrollSource {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// Meaning of an application-supplied vertical position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExternalScrollPosition {
    /// Synchronize the position while preserving native drag, coast and animation.
    Sync(f32),
    /// Set the position and terminate previous vertical velocity and animation.
    Set(f32),
    /// An application gesture owns vertical motion, including fractional positions.
    AppGesture(f32),
}

/// Current geometry and position supplied by an application-owned vertical list.
///
/// The height must equal the measured content height, including content margins.
/// This opt-in contract does not change ordinary [`ScrollArea`] behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExternalVerticalScroll {
    /// Full content height, including margins, in logical points.
    pub content_height: f32,
    /// The position and its priority over previously accumulated native motion.
    pub position: ExternalScrollPosition,
    /// Logical input lifetime, distinct from the geometry State identity.
    pub interaction_session: Id,
    /// Stable transport for this viewport's vertical list across logical owners.
    pub interaction_id: Id,
}

/// Add vertical and/or horizontal scrolling to a contained [`Ui`].
///
/// By default, scroll bars only show up when needed, i.e. when the contents
/// is larger than the container.
/// This is controlled by [`Self::scroll_bar_visibility`].
///
/// There are two flavors of scroll areas: solid and floating.
/// Solid scroll bars use up space, reducing the amount of space available
/// to the contents. Floating scroll bars float on top of the contents, covering it.
/// You can change the scroll style by changing the [`crate::style::Spacing::scroll`].
///
/// ### Coordinate system
/// * content: size of contents (generally large; that's why we want scroll bars)
/// * outer: size of scroll area including scroll bar(s)
/// * inner: excluding scroll bar(s). The area we clip the contents to. Includes `content_margin`.
///
/// If the floating scroll bars settings is turned on then `inner == outer`.
///
/// ## Example
/// ```
/// # egui::__run_test_ui(|ui| {
/// egui::ScrollArea::vertical().show(ui, |ui| {
///     // Add a lot of widgets here.
/// });
/// # });
/// ```
///
/// You can scroll to an element using [`crate::Response::scroll_to_me`], [`Ui::scroll_to_cursor`] and [`Ui::scroll_to_rect`].
///
/// ## See also
/// If you want to allow zooming, use [`crate::Scene`].
#[derive(Clone, Debug)]
#[must_use = "You should call .show()"]
pub struct ScrollArea {
    /// Do we have horizontal/vertical scrolling enabled?
    direction_enabled: Vec2b,

    auto_shrink: Vec2b,
    max_size: Vec2,
    min_scrolled_size: Vec2,
    scroll_bar_visibility: ScrollBarVisibility,
    scroll_bar_rect: Option<Rect>,
    id_salt: Option<IdSalt>,
    offset_x: Option<f32>,
    offset_y: Option<f32>,
    external_vertical_scroll: Option<ExternalVerticalScroll>,
    on_hover_cursor: Option<CursorIcon>,
    on_drag_cursor: Option<CursorIcon>,
    scroll_source: ScrollSource,
    wheel_scroll_multiplier: Vec2,

    content_margin: Option<Margin>,

    /// If true for vertical or horizontal the scroll wheel will stick to the
    /// end position until user manually changes position. It will become true
    /// again once scroll handle makes contact with end.
    stick_to_end: Vec2b,

    /// If false, `scroll_to_*` functions will not be animated
    animated: bool,
}

impl ScrollArea {
    /// Create a horizontal scroll area.
    #[inline]
    pub fn horizontal() -> Self {
        Self::new([true, false])
    }

    /// Create a vertical scroll area.
    #[inline]
    pub fn vertical() -> Self {
        Self::new([false, true])
    }

    /// Create a bi-directional (horizontal and vertical) scroll area.
    #[inline]
    pub fn both() -> Self {
        Self::new([true, true])
    }

    /// Create a scroll area where both direction of scrolling is disabled.
    /// It's unclear why you would want to do this.
    #[inline]
    pub fn neither() -> Self {
        Self::new([false, false])
    }

    /// Create a scroll area where you decide which axis has scrolling enabled.
    /// For instance, `ScrollArea::new([true, false])` enables horizontal scrolling.
    pub fn new(direction_enabled: impl Into<Vec2b>) -> Self {
        Self {
            direction_enabled: direction_enabled.into(),
            auto_shrink: Vec2b::TRUE,
            max_size: Vec2::INFINITY,
            min_scrolled_size: Vec2::splat(64.0),
            scroll_bar_visibility: Default::default(),
            scroll_bar_rect: None,
            id_salt: None,
            offset_x: None,
            offset_y: None,
            external_vertical_scroll: None,
            on_hover_cursor: None,
            on_drag_cursor: None,
            scroll_source: ScrollSource::default(),
            wheel_scroll_multiplier: Vec2::splat(1.0),
            content_margin: None,
            stick_to_end: Vec2b::FALSE,
            animated: true,
        }
    }

    /// The maximum width of the outer frame of the scroll area.
    ///
    /// Use `f32::INFINITY` if you want the scroll area to expand to fit the surrounding [`Ui`] (default).
    ///
    /// See also [`Self::auto_shrink`].
    #[inline]
    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_size.x = max_width;
        self
    }

    /// The maximum height of the outer frame of the scroll area.
    ///
    /// Use `f32::INFINITY` if you want the scroll area to expand to fit the surrounding [`Ui`] (default).
    ///
    /// See also [`Self::auto_shrink`].
    #[inline]
    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_size.y = max_height;
        self
    }

    /// The minimum width of a horizontal scroll area which requires scroll bars.
    ///
    /// The [`ScrollArea`] will only become smaller than this if the content is smaller than this
    /// (and so we don't require scroll bars).
    ///
    /// Default: `64.0`.
    #[inline]
    pub fn min_scrolled_width(mut self, min_scrolled_width: f32) -> Self {
        self.min_scrolled_size.x = min_scrolled_width;
        self
    }

    /// The minimum height of a vertical scroll area which requires scroll bars.
    ///
    /// The [`ScrollArea`] will only become smaller than this if the content is smaller than this
    /// (and so we don't require scroll bars).
    ///
    /// Default: `64.0`.
    #[inline]
    pub fn min_scrolled_height(mut self, min_scrolled_height: f32) -> Self {
        self.min_scrolled_size.y = min_scrolled_height;
        self
    }

    /// Set the visibility of both horizontal and vertical scroll bars.
    ///
    /// With `ScrollBarVisibility::VisibleWhenNeeded` (default), the scroll bar will be visible only when needed.
    #[inline]
    pub fn scroll_bar_visibility(mut self, scroll_bar_visibility: ScrollBarVisibility) -> Self {
        self.scroll_bar_visibility = scroll_bar_visibility;
        self
    }

    /// Specify within which screen-space rectangle to show the scroll bars.
    ///
    /// This can be used to move the scroll bars to a smaller region of the `ScrollArea`,
    /// for instance if you are painting a sticky header on top of it.
    #[inline]
    pub fn scroll_bar_rect(mut self, scroll_bar_rect: Rect) -> Self {
        self.scroll_bar_rect = Some(scroll_bar_rect);
        self
    }

    /// A source for the unique [`Id`], e.g. `.id_salt("second_scroll_area")` or `.id_salt(loop_index)`.
    #[inline]
    pub fn id_salt(mut self, id_salt: impl AsIdSalt) -> Self {
        self.id_salt = Some(IdSalt::new(id_salt));
        self
    }

    /// Set the horizontal and vertical scroll offset position.
    ///
    /// Positive offset means scrolling down/right.
    ///
    /// See also: [`Self::vertical_scroll_offset`], [`Self::horizontal_scroll_offset`],
    /// [`Ui::scroll_to_cursor`](crate::ui::Ui::scroll_to_cursor) and
    /// [`Response::scroll_to_me`](crate::Response::scroll_to_me)
    #[inline]
    pub fn scroll_offset(mut self, offset: Vec2) -> Self {
        self.offset_x = Some(offset.x);
        self.offset_y = Some(offset.y);
        self
    }

    /// Set the vertical scroll offset position.
    ///
    /// Positive offset means scrolling down.
    ///
    /// See also: [`Self::scroll_offset`], [`Ui::scroll_to_cursor`](crate::ui::Ui::scroll_to_cursor) and
    /// [`Response::scroll_to_me`](crate::Response::scroll_to_me)
    #[inline]
    pub fn vertical_scroll_offset(mut self, offset: f32) -> Self {
        self.offset_y = Some(offset);
        self
    }

    /// Supply the current height and application-owned position of a vertical list.
    ///
    /// Use a vertical-only area with `auto_shrink([false, false])`. The caller must
    /// use the same minimum viewport height and margins in its geometry calculation.
    /// Explicit application clamp/snap corrections must use [`ExternalScrollPosition::Set`],
    /// even when the corrected position is already inside the current bounds.
    pub fn external_vertical_scroll(mut self, scroll: ExternalVerticalScroll) -> Self {
        self.external_vertical_scroll = Some(scroll);
        self
    }

    /// Set the horizontal scroll offset position.
    ///
    /// Positive offset means scrolling right.
    ///
    /// See also: [`Self::scroll_offset`], [`Ui::scroll_to_cursor`](crate::ui::Ui::scroll_to_cursor) and
    /// [`Response::scroll_to_me`](crate::Response::scroll_to_me)
    #[inline]
    pub fn horizontal_scroll_offset(mut self, offset: f32) -> Self {
        self.offset_x = Some(offset);
        self
    }

    /// Set the cursor used when the mouse pointer is hovering over the [`ScrollArea`].
    ///
    /// Only applies if [`Self::scroll_source()`] has set [`ScrollSource::drag`] to `true`.
    ///
    /// Any changes to the mouse cursor made within the contents of the [`ScrollArea`] will
    /// override this setting.
    #[inline]
    pub fn on_hover_cursor(mut self, cursor: CursorIcon) -> Self {
        self.on_hover_cursor = Some(cursor);
        self
    }

    /// Set the cursor used when the [`ScrollArea`] is being dragged.
    ///
    /// Only applies if [`Self::scroll_source()`] has set [`ScrollSource::drag`] to `true`.
    ///
    /// Any changes to the mouse cursor made within the contents of the [`ScrollArea`] will
    /// override this setting.
    #[inline]
    pub fn on_drag_cursor(mut self, cursor: CursorIcon) -> Self {
        self.on_drag_cursor = Some(cursor);
        self
    }

    /// Turn on/off scrolling on the horizontal axis.
    #[inline]
    pub fn hscroll(mut self, hscroll: bool) -> Self {
        self.direction_enabled[0] = hscroll;
        self
    }

    /// Turn on/off scrolling on the vertical axis.
    #[inline]
    pub fn vscroll(mut self, vscroll: bool) -> Self {
        self.direction_enabled[1] = vscroll;
        self
    }

    /// Turn on/off scrolling on the horizontal/vertical axes.
    ///
    /// You can pass in `false`, `true`, `[false, true]` etc.
    #[inline]
    pub fn scroll(mut self, direction_enabled: impl Into<Vec2b>) -> Self {
        self.direction_enabled = direction_enabled.into();
        self
    }

    /// Control the scrolling behavior.
    ///
    /// * If `true` (default), the scroll area will respond to user scrolling.
    /// * If `false`, the scroll area will not respond to user scrolling.
    ///
    /// This can be used, for example, to optionally freeze scrolling while the user
    /// is typing text in a [`crate::TextEdit`] widget contained within the scroll area.
    ///
    /// What sources does the [`ScrollArea`] use for scrolling the contents.
    #[inline]
    pub fn scroll_source(mut self, scroll_source: ScrollSource) -> Self {
        self.scroll_source = scroll_source;
        self
    }

    /// The scroll amount caused by a mouse wheel scroll is multiplied by this amount.
    ///
    /// Independent for each scroll direction. Defaults to `Vec2{x: 1.0, y: 1.0}`.
    ///
    /// This can invert or effectively disable mouse scrolling.
    #[inline]
    pub fn wheel_scroll_multiplier(mut self, multiplier: Vec2) -> Self {
        self.wheel_scroll_multiplier = multiplier;
        self
    }

    /// For each axis, should the containing area shrink if the content is small?
    ///
    /// * If `true`, egui will add blank space outside the scroll area.
    /// * If `false`, egui will add blank space inside the scroll area.
    ///
    /// Default: `true`.
    #[inline]
    pub fn auto_shrink(mut self, auto_shrink: impl Into<Vec2b>) -> Self {
        self.auto_shrink = auto_shrink.into();
        self
    }

    /// Should the scroll area animate `scroll_to_*` functions?
    ///
    /// Default: `true`.
    #[inline]
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }

    /// Is any scrolling enabled?
    pub(crate) fn is_any_scroll_enabled(&self) -> bool {
        self.direction_enabled[0] || self.direction_enabled[1]
    }

    /// Extra margin added around the contents.
    ///
    /// The scroll bars will be either on top of this margin, or outside of it,
    /// depending on the value of [`crate::style::ScrollStyle::floating`].
    ///
    /// Default: [`crate::style::ScrollStyle::content_margin`].
    #[inline]
    pub fn content_margin(mut self, margin: impl Into<Margin>) -> Self {
        self.content_margin = Some(margin.into());
        self
    }

    /// The scroll handle will stick to the rightmost position even while the content size
    /// changes dynamically. This can be useful to simulate text scrollers coming in from right
    /// hand side. The scroll handle remains stuck until user manually changes position. Once "unstuck"
    /// it will remain focused on whatever content viewport the user left it on. If the scroll
    /// handle is dragged all the way to the right it will again become stuck and remain there
    /// until manually pulled from the end position.
    #[inline]
    pub fn stick_to_right(mut self, stick: bool) -> Self {
        self.stick_to_end[0] = stick;
        self
    }

    /// The scroll handle will stick to the bottom position even while the content size
    /// changes dynamically. This can be useful to simulate terminal UIs or log/info scrollers.
    /// The scroll handle remains stuck until user manually changes position. Once "unstuck"
    /// it will remain focused on whatever content viewport the user left it on. If the scroll
    /// handle is dragged to the bottom it will again become stuck and remain there until manually
    /// pulled from the end position.
    #[inline]
    pub fn stick_to_bottom(mut self, stick: bool) -> Self {
        self.stick_to_end[1] = stick;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExternalPressStamp {
    frame: u64,
    ordinal: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalCaptureRole {
    Background,
    VerticalBar,
}

/// The existing processed-frame stamp belongs to the displayed owner, which
/// can differ from a held press's original owner after a list switch.
#[derive(Clone, Copy, Debug)]
struct ExternalProcessedFrame {
    number: u64,
    session: Id,
}

#[derive(Clone, Copy, Debug)]
enum ExternalCapturePhase {
    Held {
        position: Pos2,
        frame: ExternalProcessedFrame,
    },
    Released {
        frame: ExternalProcessedFrame,
    },
}

#[derive(Clone, Copy, Debug)]
enum ExternalScrollCapture {
    Idle {
        frame: Option<ExternalProcessedFrame>,
    },
    Sequence {
        press_stamp: ExternalPressStamp,
        session: Id,
        role: ExternalCaptureRole,
        button: crate::PointerButton,
        phase: ExternalCapturePhase,
    },
}

impl Default for ExternalScrollCapture {
    fn default() -> Self {
        Self::Idle { frame: None }
    }
}

impl ExternalScrollCapture {
    fn processed_frame(self) -> Option<ExternalProcessedFrame> {
        match self {
            Self::Idle { frame } => frame,
            Self::Sequence { phase, .. } => Some(match phase {
                ExternalCapturePhase::Held { frame, .. }
                | ExternalCapturePhase::Released { frame } => frame,
            }),
        }
    }

    fn frame(self) -> Option<u64> {
        self.processed_frame().map(|frame| frame.number)
    }

    fn processed_for(self, frame: u64, session: Id) -> bool {
        self.processed_frame()
            .is_some_and(|processed| processed.number == frame && processed.session == session)
    }

    fn contains_stamp(self, candidate: ExternalPressStamp) -> bool {
        matches!(self, Self::Sequence { press_stamp, .. }
            if press_stamp.frame == candidate.frame && candidate.ordinal <= press_stamp.ordinal)
    }
}

#[derive(Clone, Copy, Debug)]
struct ExternalPress {
    stamp: ExternalPressStamp,
    position: Pos2,
    button: crate::PointerButton,
}

#[derive(Clone, Copy, Debug, Default)]
struct ExternalCaptureInput {
    background_delta: Option<Vec2>,
    background_released: bool,
    bar_position: Option<Pos2>,
    new_press: Option<(ExternalCaptureRole, ExternalPress)>,
    short_press: Option<ExternalPress>,
    replay: bool,
}

fn external_capture_id(transport: Id) -> Id {
    transport.with("external_scroll_capture")
}

fn external_press_in_rect(ui: &Ui, rect: Rect, position: Pos2) -> bool {
    // Match egui's near-widget hit radius as well as its clipped interaction
    // rectangle. Final Memory holding remains mandatory; proximity alone
    // never authorizes a press belonging to another widget.
    rect.intersect(ui.clip_rect()).distance_to_pos(position)
        <= ui.style().interaction.interact_radius
}

/// Observe each viewport input once. Final egui holding proves only the last
/// unfinished press; completed intervals are never assigned from that evidence.
fn external_capture_input(
    ui: &Ui,
    external: ExternalVerticalScroll,
    background_rect: Rect,
    background_enabled: bool,
) -> ExternalCaptureInput {
    use crate::PointerButton::{Extra1, Extra2, Middle, Primary, Secondary};
    use crate::input_state::PointerEvent;

    let ctx = ui.ctx();
    let frame = ctx.cumulative_frame_nr();
    let processed_frame = ExternalProcessedFrame {
        number: frame,
        session: external.interaction_session,
    };
    let capture_id = external_capture_id(external.interaction_id);
    let mut capture = ctx
        .data(|data| data.get_temp::<ExternalScrollCapture>(capture_id))
        .unwrap_or_default();
    if capture.frame() == Some(frame) {
        return ExternalCaptureInput {
            replay: true,
            ..Default::default()
        };
    }
    let (events, mut down) = ctx.input(|input| {
        (
            input.pointer.pointer_events.clone(),
            [Primary, Secondary, Middle, Extra1, Extra2]
                .map(|button| input.pointer.button_down(button)),
        )
    });
    // Recover button levels at the beginning of the ordered batch. An
    // additional button while another is down cannot acquire a new lifetime.
    for event in events.iter().rev() {
        match event {
            PointerEvent::Pressed { button, .. } => down[*button as usize] = false,
            PointerEvent::Released { button, .. } => down[*button as usize] = true,
            PointerEvent::Moved(_) => {}
        }
    }
    if let ExternalScrollCapture::Sequence {
        button,
        phase: ExternalCapturePhase::Held { .. },
        ..
    } = capture
    {
        down[button as usize] = true;
    }
    let initial_down = down;
    let mut ordinal = 0;
    let mut tail = None;
    let mut short_press = None;
    for event in &events {
        match *event {
            PointerEvent::Pressed { position, button } => {
                if !down.iter().any(|down| *down) {
                    tail = Some(ExternalPress {
                        stamp: ExternalPressStamp { frame, ordinal },
                        position,
                        button,
                    });
                }
                down[button as usize] = true;
                ordinal += 1;
            }
            PointerEvent::Released { button, .. } => {
                if tail.is_some_and(|press| press.button == button) {
                    short_press = tail.take();
                }
                down[button as usize] = false;
            }
            PointerEvent::Moved(_) => {}
        }
    }
    let holding = ctx.memory(|memory| {
        let interaction = memory.interaction();
        let bar = external.interaction_id.with(1_usize);
        if interaction.potential_click_id == Some(bar) || interaction.potential_drag_id == Some(bar)
        {
            Some(ExternalCaptureRole::VerticalBar)
        } else if interaction.potential_drag_id == Some(external.interaction_id.with("area")) {
            Some(ExternalCaptureRole::Background)
        } else {
            None
        }
    });
    let mut result = ExternalCaptureInput {
        short_press,
        ..Default::default()
    };
    down = initial_down;
    ordinal = 0;
    for event in &events {
        match *event {
            PointerEvent::Pressed { position, button } => {
                if tail.is_some_and(|press| press.stamp.ordinal == ordinal)
                    && !capture.contains_stamp(tail.unwrap().stamp)
                    && let Some(role) = holding
                    && ui.is_enabled()
                    && (role == ExternalCaptureRole::VerticalBar
                        || (background_enabled
                            && external_press_in_rect(ui, background_rect, position)))
                {
                    let press = tail.unwrap();
                    capture = ExternalScrollCapture::Sequence {
                        press_stamp: press.stamp,
                        session: external.interaction_session,
                        role,
                        button,
                        phase: ExternalCapturePhase::Held {
                            position,
                            frame: processed_frame,
                        },
                    };
                    result.new_press = Some((role, press));
                    // A new sequence cannot inherit the old release velocity.
                    result.background_released = false;
                    result.bar_position = None;
                    result.background_delta =
                        (role == ExternalCaptureRole::Background).then_some(Vec2::ZERO);
                }
                down[button as usize] = true;
                ordinal += 1;
            }
            PointerEvent::Moved(position) => {
                if let ExternalScrollCapture::Sequence {
                    session,
                    role,
                    button,
                    phase:
                        ExternalCapturePhase::Held {
                            position: previous, ..
                        },
                    ..
                } = &mut capture
                {
                    if down[*button as usize]
                        && *session == external.interaction_session
                        && *role == ExternalCaptureRole::Background
                    {
                        *result.background_delta.get_or_insert(Vec2::ZERO) += position - *previous;
                    }
                    *previous = position;
                }
            }
            PointerEvent::Released { button, .. } => {
                if let ExternalScrollCapture::Sequence {
                    session,
                    role,
                    button: owner_button,
                    phase,
                    ..
                } = &mut capture
                    && *owner_button == button
                    && matches!(phase, ExternalCapturePhase::Held { .. })
                {
                    result.background_released = *session == external.interaction_session
                        && *role == ExternalCaptureRole::Background;
                    if *session == external.interaction_session
                        && *role == ExternalCaptureRole::VerticalBar
                        && let ExternalCapturePhase::Held { position, .. } = *phase
                    {
                        result.bar_position = Some(position);
                    }
                    result.background_delta = None;
                    *phase = ExternalCapturePhase::Released {
                        frame: processed_frame,
                    };
                }
                down[button as usize] = false;
            }
        }
    }
    if let ExternalScrollCapture::Sequence { role, phase, .. } = &mut capture
        && matches!(phase, ExternalCapturePhase::Held { .. })
        && holding != Some(*role)
    {
        // Escape, pointer loss, or another egui capture ends this sequence's
        // authority. A later move cannot revive a still-held old press.
        *phase = ExternalCapturePhase::Released {
            frame: processed_frame,
        };
        result.background_delta = None;
        result.background_released = false;
        result.bar_position = None;
    }
    match &mut capture {
        ExternalScrollCapture::Idle { frame: processed } => *processed = Some(processed_frame),
        ExternalScrollCapture::Sequence {
            session,
            role,
            button,
            phase,
            ..
        } => match phase {
            ExternalCapturePhase::Held {
                position,
                frame: processed,
            } => {
                *processed = processed_frame;
                if down[*button as usize] && *session == external.interaction_session {
                    if *role == ExternalCaptureRole::Background {
                        result.background_delta.get_or_insert(Vec2::ZERO);
                    } else {
                        result.bar_position = Some(*position);
                    }
                }
            }
            ExternalCapturePhase::Released { frame: processed } => *processed = processed_frame,
        },
    }
    if !background_enabled {
        result.background_delta = None;
        result.background_released = false;
    }
    ctx.data_mut(|data| data.insert_temp(capture_id, capture));
    result
}

fn finish_external_bar_press(
    ctx: &Context,
    external: ExternalVerticalScroll,
    press: ExternalPress,
) {
    let frame = ctx.cumulative_frame_nr();
    let processed_frame = ExternalProcessedFrame {
        number: frame,
        session: external.interaction_session,
    };
    ctx.data_mut(|data| {
        data.insert_temp(
            external_capture_id(external.interaction_id),
            ExternalScrollCapture::Sequence {
                press_stamp: press.stamp,
                session: external.interaction_session,
                role: ExternalCaptureRole::VerticalBar,
                button: press.button,
                phase: ExternalCapturePhase::Released {
                    frame: processed_frame,
                },
            },
        );
    });
}

struct Prepared {
    id: Id,
    state: State,

    auto_shrink: Vec2b,

    /// Does this `ScrollArea` have horizontal/vertical scrolling enabled?
    direction_enabled: Vec2b,

    /// Smoothly interpolated boolean of whether or not to show the scroll bars.
    show_bars_factor: Vec2,

    /// How much horizontal and vertical space are used up by the
    /// width of the vertical bar, and the height of the horizontal bar?
    ///
    /// This is always zero for floating scroll bars.
    ///
    /// Note that this is a `yx` swizzling of [`Self::show_bars_factor`]
    /// times the maximum bar with.
    /// That's because horizontal scroll uses up vertical space,
    /// and vice versa.
    current_bar_use: Vec2,

    scroll_bar_visibility: ScrollBarVisibility,
    scroll_bar_rect: Option<Rect>,

    /// Where on the screen the content is (excludes scroll bars; includes `content_margin`).
    inner_rect: Rect,

    content_ui: Ui,

    /// Relative coordinates: the offset and size of the view of the inner UI.
    /// `viewport.min == ZERO` means we scrolled to the top.
    viewport: Rect,

    scroll_source: ScrollSource,
    wheel_scroll_multiplier: Vec2,
    stick_to_end: Vec2b,

    /// If there was a scroll target before the [`ScrollArea`] was added this frame, it's
    /// not for us to handle so we save it and restore it after this [`ScrollArea`] is done.
    saved_scroll_target: [Option<pass_state::ScrollTarget>; 2],

    /// The response from dragging the background (if enabled)
    background_drag_response: Option<Response>,

    animated: bool,

    /// Along which axes the offset is deliberately outside the valid range this frame
    /// (rubber-banding), and so should not be clamped.
    rubber_banding: Vec2b,

    /// Is there an enclosing [`ScrollArea`]?
    has_scrolling_ancestor: bool,

    /// Per axis. Y bounds are current for externally controlled vertical lists.
    axis_physics: [AxisPhysics; 2],

    external_vertical_scroll: Option<ExternalVerticalScroll>,
    external_capture_input: ExternalCaptureInput,
}

fn scroll_content_ui(
    ui: &mut Ui,
    inner_rect: Rect,
    offset: Vec2,
    current_bar_use: Vec2,
    direction_enabled: Vec2b,
) -> Ui {
    // Try to fit the content within the viewport before enabling scrolling.
    let content_max_rect = Rect::from_min_size(inner_rect.min - offset, inner_rect.size())
        .round_to_pixels(ui.pixels_per_point())
        .round_ui();
    let mut content_ui = ui.new_child(
        UiBuilder::new()
            .ui_stack_info(UiStackInfo::new(UiKind::ScrollArea))
            .max_rect(content_max_rect),
    );
    let mut content_clip_rect = ui.clip_rect();
    for d in 0..2 {
        if direction_enabled[d] {
            content_clip_rect.min[d] = inner_rect.min[d];
            content_clip_rect.max[d] = inner_rect.max[d];
        } else {
            content_clip_rect.max[d] = ui.clip_rect().max[d] - current_bar_use[d];
        }
    }
    content_ui.set_clip_rect(content_clip_rect.intersect(ui.clip_rect()));
    content_ui
}

impl ScrollArea {
    fn begin(self, ui: &mut Ui) -> Prepared {
        let Self {
            direction_enabled,
            auto_shrink,
            max_size,
            min_scrolled_size,
            scroll_bar_visibility,
            scroll_bar_rect,
            id_salt,
            offset_x,
            offset_y,
            external_vertical_scroll,
            on_hover_cursor,
            on_drag_cursor,
            scroll_source,
            wheel_scroll_multiplier,
            content_margin: _, // Used elsewhere
            stick_to_end,
            animated,
        } = self;

        let ctx = ui.ctx().clone();

        let id_salt = id_salt.unwrap_or_else(|| IdSalt::new("scroll_area"));
        let id = ui.make_persistent_id(id_salt);
        ctx.check_for_id_clash(
            id,
            Rect::from_min_size(ui.available_rect_before_wrap().min, Vec2::ZERO),
            "ScrollArea",
        );
        let mut state = State::load(&ctx, id).unwrap_or_default();

        let association = external_vertical_scroll.map_or(ScrollAssociation::Native, |external| {
            ScrollAssociation::ExternalVertical(external.interaction_session)
        });
        let association_changed = state.association != association;
        if association_changed {
            state.vel.y = 0.0;
            state.offset_target[1] = None;
            state.scroll_start_offset_from_top_left[1] = None;
            state.bounced_this_momentum.y = false;
            state.association = association;
        }

        state.offset.x = offset_x.unwrap_or(state.offset.x);
        state.offset.y = offset_y.unwrap_or(state.offset.y);

        let show_bars: Vec2b = match scroll_bar_visibility {
            ScrollBarVisibility::AlwaysHidden => Vec2b::FALSE,
            ScrollBarVisibility::VisibleWhenNeeded => state.show_scroll,
            ScrollBarVisibility::AlwaysVisible => direction_enabled,
        };

        let show_bars_factor = Vec2::new(
            ctx.animate_bool_responsive(id.with("h"), show_bars[0]),
            ctx.animate_bool_responsive(id.with("v"), show_bars[1]),
        );

        let scroll_style = ui.spacing().scroll;
        let current_bar_use = if scroll_style.floating {
            show_bars.to_vec2().yx() * scroll_style.allocated_width()
        } else {
            show_bars_factor.yx() * scroll_style.allocated_width()
        };

        let available_outer = ui.available_rect_before_wrap();

        let outer_size = available_outer.size().at_most(max_size);

        let inner_size = {
            let mut inner_size = outer_size - current_bar_use;

            // Don't go so far that we shrink to zero.
            // In particular, if we put a [`ScrollArea`] inside of a [`ScrollArea`], the inner
            // one shouldn't collapse into nothingness.
            // See https://github.com/emilk/egui/issues/1097
            for d in 0..2 {
                if direction_enabled[d] {
                    inner_size[d] = inner_size[d].max(min_scrolled_size[d]);
                }
            }
            inner_size
        };

        let inner_rect = Rect::from_min_size(available_outer.min, inner_size);

        if let Some(external) = external_vertical_scroll {
            debug_assert!(direction_enabled == Vec2b::new(false, true));
            debug_assert!(!auto_shrink.y);
            debug_assert!(external.content_height.is_finite() && external.content_height >= 0.0);
            let previous_max_offset = state.max_offset.y;
            state.max_offset.y = external.content_height - inner_size.y;
            state.content_is_too_large.y = inner_size.y.ceil() < external.content_height;
            let (position, end_previous_motion) = match external.position {
                ExternalScrollPosition::Sync(position) => (position, false),
                ExternalScrollPosition::Set(position)
                | ExternalScrollPosition::AppGesture(position) => (position, true),
            };
            debug_assert!(position.is_finite());
            // App readback intentionally ignores sub-row native movement. On
            // a same-owner retry Sync must retain the first pass's result;
            // injecting that older canonical value would erase valid motion
            // before the capture owner skips the already-processed input.
            let sync_replay = !end_previous_motion
                && !association_changed
                && ctx
                    .data(|data| {
                        data.get_temp::<ExternalScrollCapture>(external_capture_id(
                            external.interaction_id,
                        ))
                    })
                    .is_some_and(|capture| {
                        capture
                            .processed_for(ctx.cumulative_frame_nr(), external.interaction_session)
                    });
            let position = if sync_replay {
                // Retain legitimate first-pass rubber-band displacement too.
                // Only a changed extent may clamp that native result here.
                (previous_max_offset != state.max_offset.y).then_some(state.offset.y)
            } else {
                Some(position)
            };
            if let Some(position) = position {
                state.offset.y = scroll_bounds(state.max_offset.y).clamp(position);
                if end_previous_motion || state.offset.y != position {
                    state.vel.y = 0.0;
                    state.offset_target[1] = None;
                }
            }
        }
        let app_gesture = external_vertical_scroll.is_some_and(|external| {
            matches!(external.position, ExternalScrollPosition::AppGesture(_))
        });

        // The native path keeps its original pre-physics content placement. Only
        // the opt-in path constructs content from the position after physics.
        let native_content = external_vertical_scroll.is_none().then(|| {
            (
                scroll_content_ui(
                    ui,
                    inner_rect,
                    state.offset,
                    current_bar_use,
                    direction_enabled,
                ),
                Rect::from_min_size(Pos2::ZERO + state.offset, inner_size),
            )
        });

        // Is there an enclosing scroll area? If so, we let the scroll wheel chain to it
        // when we're at the edge.
        let has_scrolling_ancestor = has_scrolling_ancestor(ui);

        let dt = ui.input(|i| i.stable_dt).at_most(0.1);

        let kinetic_style = ui.spacing().scroll.kinetic;
        let mut rubber_banding = Vec2b::FALSE;

        let axis_physics: [AxisPhysics; 2] = core::array::from_fn(|d| AxisPhysics {
            bounds: scroll_bounds(state.max_offset[d]),
            viewport_extent: inner_size[d],
            style: KineticScrollStyle {
                // Only rubber-band along axes that actually have something to scroll:
                rubber_band: kinetic_style.rubber_band
                    && direction_enabled[d]
                    && state.content_is_too_large[d],
                ..kinetic_style
            },
        });

        let background_drag_response = if scroll_source.drag.enabled(ui.ctx())
            && ui.is_enabled()
            && state.content_is_too_large.any()
        {
            // Drag contents to scroll (for touch screens mostly).
            // We must do this BEFORE adding content to the `ScrollArea`,
            // or we will steal input from the widgets we contain.
            let content_response_option = external_vertical_scroll
                .map_or(state.interact_rect, |_| Some(inner_rect))
                .map(|rect| {
                    ui.interact(
                        rect,
                        external_vertical_scroll
                            .map_or(id, |external| external.interaction_id)
                            .with("area"),
                        Sense::DRAG,
                    )
                });

            // Set the desired mouse cursors.
            if let Some(response) = &content_response_option {
                if response.dragged()
                    && let Some(cursor) = on_drag_cursor
                {
                    ui.set_cursor_icon(cursor);
                } else if response.hovered()
                    && let Some(cursor) = on_hover_cursor
                {
                    ui.set_cursor_icon(cursor);
                }
            }

            content_response_option
        } else {
            None
        };

        let external_capture_input =
            external_vertical_scroll.map_or_else(ExternalCaptureInput::default, |external| {
                external_capture_input(ui, external, inner_rect, background_drag_response.is_some())
            });
        let skip_native_y = app_gesture || external_capture_input.replay;
        if skip_native_y {
            // No second physics step is needed, but end must still preserve a
            // legitimate first-pass rubber-band result instead of clamping it.
            rubber_banding.y = axis_physics[1].is_rubber_banding(state.offset.y);
        }
        if external_capture_input.new_press.is_some() {
            state.vel.y = 0.0;
            state.offset_target[1] = None;
            state.scroll_start_offset_from_top_left[1] = None;
        }

        let background_delta = if external_vertical_scroll.is_some() {
            external_capture_input.background_delta
        } else {
            background_drag_response
                .as_ref()
                .filter(|response| response.dragged())
                .map(|_| ui.input(|input| input.pointer.delta()))
        };
        if let Some(pointer_delta) = background_delta {
            for d in 0..2 {
                if direction_enabled[d] && !(skip_native_y && d == 1) {
                    let physics = &axis_physics[d];
                    // Offset has an inverted coordinate system compared to the pointer:
                    state.offset[d] = physics.drag(state.offset[d], -pointer_delta[d]);
                    rubber_banding[d] = physics.is_rubber_banding(state.offset[d]);
                    state.scroll_stuck_to_end[d] = false;
                    state.offset_target[d] = None;
                }
            }
        } else {
            // Apply the cursor velocity to the scroll area when the user releases the drag.
            if if external_vertical_scroll.is_some() {
                external_capture_input.background_released
            } else {
                background_drag_response
                    .as_ref()
                    .is_some_and(|response| response.drag_stopped())
            } {
                // Offset has an inverted coordinate system compared to the pointer:
                state.vel =
                    -(direction_enabled.to_vec2() * ui.input(|input| input.pointer.velocity()));
                if skip_native_y {
                    state.vel.y = 0.0;
                }
            }

            // While the fingers are still on the trackpad,
            // hold the overscroll instead of springing back, like macOS does.
            // The wheel events are applied in `end`.
            let fingers_on_trackpad =
                ui.input(|i| i.scroll_source()) == Some(MouseWheelSource::Trackpad);
            let hold_overscroll = fingers_on_trackpad && ui.rect_contains_pointer(inner_rect);

            for d in 0..2 {
                if skip_native_y && d == 1 {
                    continue;
                }
                let physics = &axis_physics[d];
                if physics.step(&mut state.offset[d], &mut state.vel[d], dt, hold_overscroll) {
                    ctx.request_repaint();
                }
                rubber_banding[d] = physics.is_rubber_banding(state.offset[d]);
            }
        }

        // Scroll with an animation if we have a target offset (that hasn't been cleared by the code
        // above).
        for d in 0..2 {
            if skip_native_y && d == 1 {
                continue;
            }
            if let Some(scroll_target) = state.offset_target[d] {
                state.vel[d] = 0.0;

                if (state.offset[d] - scroll_target.target_offset).abs() < 1.0 {
                    // Arrived
                    state.offset[d] = scroll_target.target_offset;
                    state.offset_target[d] = None;
                } else {
                    // Move towards target
                    let t = emath::interpolation_factor(
                        scroll_target.animation_time_span,
                        ui.input(|i| i.time),
                        dt,
                        emath::ease_in_ease_out,
                    );
                    if t < 1.0 {
                        state.offset[d] =
                            emath::lerp(state.offset[d]..=scroll_target.target_offset, t);
                        ctx.request_repaint();
                    } else {
                        // Arrived
                        state.offset[d] = scroll_target.target_offset;
                        state.offset_target[d] = None;
                    }
                }
            }
        }

        let (content_ui, viewport) = native_content.unwrap_or_else(|| {
            (
                scroll_content_ui(
                    ui,
                    inner_rect,
                    state.offset,
                    current_bar_use,
                    direction_enabled,
                ),
                Rect::from_min_size(Pos2::ZERO + state.offset, inner_size),
            )
        });

        let saved_scroll_target = content_ui
            .ctx()
            .pass_state_mut(|state| core::mem::take(&mut state.scroll_target));

        Prepared {
            id,
            state,
            auto_shrink,
            direction_enabled,
            show_bars_factor,
            current_bar_use,
            scroll_bar_visibility,
            scroll_bar_rect,
            inner_rect,
            content_ui,
            viewport,
            scroll_source,
            wheel_scroll_multiplier,
            stick_to_end,
            saved_scroll_target,
            background_drag_response,
            animated,
            rubber_banding,
            has_scrolling_ancestor,
            axis_physics,
            external_vertical_scroll,
            external_capture_input,
        }
    }

    /// Show the [`ScrollArea`], and add the contents to the viewport.
    ///
    /// If the inner area can be very long, consider using [`Self::show_rows`] instead.
    pub fn show<R>(
        self,
        ui: &mut Ui,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> ScrollAreaOutput<R> {
        self.show_viewport_dyn(ui, Box::new(|ui, _viewport| add_contents(ui)))
    }

    /// Efficiently show only the visible part of a large number of rows.
    ///
    /// ```
    /// # egui::__run_test_ui(|ui| {
    /// let text_style = egui::TextStyle::Body;
    /// let row_height = ui.text_style_height(&text_style);
    /// // let row_height = ui.spacing().interact_size.y; // if you are adding buttons instead of labels.
    /// let total_rows = 10_000;
    /// egui::ScrollArea::vertical().show_rows(ui, row_height, total_rows, |ui, row_range| {
    ///     for row in row_range {
    ///         let text = format!("Row {}/{}", row + 1, total_rows);
    ///         ui.label(text);
    ///     }
    /// });
    /// # });
    /// ```
    pub fn show_rows<R>(
        self,
        ui: &mut Ui,
        row_height_sans_spacing: f32,
        total_rows: usize,
        add_contents: impl FnOnce(&mut Ui, core::ops::Range<usize>) -> R,
    ) -> ScrollAreaOutput<R> {
        let spacing = ui.spacing().item_spacing;
        let row_height_with_spacing = row_height_sans_spacing + spacing.y;
        self.show_viewport(ui, |ui, viewport| {
            ui.set_height((row_height_with_spacing * total_rows as f32 - spacing.y).at_least(0.0));

            let mut min_row = (viewport.min.y / row_height_with_spacing).floor() as usize;
            let mut max_row = (viewport.max.y / row_height_with_spacing).ceil() as usize + 1;
            if max_row > total_rows {
                let diff = max_row.saturating_sub(min_row);
                max_row = total_rows;
                min_row = total_rows.saturating_sub(diff);
            }

            let y_min = ui.max_rect().top() + min_row as f32 * row_height_with_spacing;
            let y_max = ui.max_rect().top() + max_row as f32 * row_height_with_spacing;

            let rect = Rect::from_x_y_ranges(ui.max_rect().x_range(), y_min..=y_max);

            ui.scope_builder(UiBuilder::new().max_rect(rect), |viewport_ui| {
                viewport_ui.skip_ahead_auto_ids(min_row); // Make sure we get consistent IDs.
                add_contents(viewport_ui, min_row..max_row)
            })
            .inner
        })
    }

    /// This can be used to only paint the visible part of the contents.
    ///
    /// `add_contents` is given the viewport rectangle, which is the relative view of the content.
    /// So if the passed rect has min = zero, then show the top left content (the user has not scrolled).
    pub fn show_viewport<R>(
        self,
        ui: &mut Ui,
        add_contents: impl FnOnce(&mut Ui, Rect) -> R,
    ) -> ScrollAreaOutput<R> {
        self.show_viewport_dyn(ui, Box::new(add_contents))
    }

    fn show_viewport_dyn<'c, R>(
        self,
        ui: &mut Ui,
        add_contents: Box<dyn FnOnce(&mut Ui, Rect) -> R + 'c>,
    ) -> ScrollAreaOutput<R> {
        let margin = self
            .content_margin
            .unwrap_or_else(|| ui.spacing().scroll.content_margin);

        let mut prepared = self.begin(ui);
        let id = prepared.id;
        let inner_rect = prepared.inner_rect;

        let inner = crate::Frame::NONE
            .inner_margin(margin)
            .show(&mut prepared.content_ui, |ui| {
                add_contents(ui, prepared.viewport)
            })
            .inner;

        let (content_size, state) = prepared.end(ui);
        ScrollAreaOutput {
            inner,
            id,
            state,
            content_size,
            inner_rect,
        }
    }
}

impl Prepared {
    /// Returns content size and state
    fn end(self, ui: &mut Ui) -> (Vec2, State) {
        let Self {
            id,
            mut state,
            inner_rect,
            auto_shrink,
            direction_enabled,
            mut show_bars_factor,
            current_bar_use,
            scroll_bar_visibility,
            scroll_bar_rect,
            content_ui,
            viewport: _,
            scroll_source,
            wheel_scroll_multiplier,
            stick_to_end,
            saved_scroll_target,
            background_drag_response,
            animated,
            mut rubber_banding,
            has_scrolling_ancestor,
            axis_physics,
            external_vertical_scroll,
            external_capture_input,
        } = self;

        let content_size = content_ui.min_size();
        if let Some(external) = external_vertical_scroll {
            // Content placement is pixel-rounded; tolerate only arithmetic
            // noise in the measured extent, not an omitted margin or row.
            debug_assert!(
                (content_size.y - external.content_height).abs()
                    <= 0.01_f32.max(external.content_height.abs() * f32::EPSILON * 4.0),
                "external vertical content height differs: declared={}, measured={}",
                external.content_height,
                content_size.y,
            );
        }
        let app_gesture = external_vertical_scroll.is_some_and(|external| {
            matches!(external.position, ExternalScrollPosition::AppGesture(_))
        });

        let skip_native_y = app_gesture || external_capture_input.replay;

        let mut had_explicit_scroll_adjustment = Vec2b::new(
            false,
            external_vertical_scroll.is_some_and(|external| {
                !matches!(external.position, ExternalScrollPosition::Sync(_))
            }),
        );

        for d in 0..2 {
            // We always take both scroll targets regardless of which scroll axes are enabled. This
            // is to avoid them leaking to other scroll areas.
            let scroll_target = content_ui
                .ctx()
                .pass_state_mut(|state| state.scroll_target[d].take());

            if direction_enabled[d] {
                let (scroll_delta, scroll_animation) = content_ui.ctx().pass_state_mut(|state| {
                    (
                        core::mem::take(&mut state.scroll_delta.0[d]),
                        state.scroll_delta.1,
                    )
                });
                if skip_native_y && d == 1 {
                    continue;
                }

                // PassState::scroll_delta is inverted from the way we apply the delta, so we need to negate it.
                let mut delta = -scroll_delta;
                let mut animation = scroll_animation;

                if let Some(target) = scroll_target {
                    let pass_state::ScrollTarget {
                        range,
                        align,
                        animation: animation_update,
                    } = target;
                    let min = content_ui.min_rect().min[d];
                    let clip_rect = content_ui.clip_rect();
                    let visible_range = min..=min + clip_rect.size()[d];
                    let (start, end) = (range.min, range.max);
                    let clip_start = clip_rect.min[d];
                    let clip_end = clip_rect.max[d];
                    let mut spacing = content_ui.spacing().item_spacing[d];

                    let delta_update = if let Some(align) = align {
                        let center_factor = align.to_factor();

                        let offset =
                            lerp(range, center_factor) - lerp(visible_range, center_factor);

                        // Depending on the alignment we need to add or subtract the spacing
                        spacing *= remap(center_factor, 0.0..=1.0, -1.0..=1.0);

                        offset + spacing - state.offset[d]
                    } else if start < clip_start && end < clip_end {
                        -(clip_start - start + spacing).min(clip_end - end - spacing)
                    } else if end > clip_end && start > clip_start {
                        (end - clip_end + spacing).min(start - clip_start - spacing)
                    } else {
                        // Ui is already in view, no need to adjust scroll.
                        0.0
                    };

                    animation = animation_update;
                    delta += delta_update;
                }

                if delta != 0.0 {
                    let target_offset = state.offset[d] + delta;

                    if !animated {
                        state.offset[d] = target_offset;
                    } else if let Some(animation) = &mut state.offset_target[d] {
                        // For instance: the user is continuously calling `ui.scroll_to_cursor`,
                        // so we don't want to reset the animation, but perhaps update the target:
                        animation.target_offset = target_offset;
                    } else {
                        // The further we scroll, the more time we take.
                        let now = ui.input(|i| i.time);
                        let animation_duration = (delta.abs() / animation.points_per_second)
                            .clamp(animation.duration.min, animation.duration.max);
                        state.offset_target[d] = Some(ScrollingToTarget {
                            animation_time_span: (now, now + animation_duration as f64),
                            target_offset,
                        });
                    }
                    ui.request_repaint();
                }

                if delta != 0.0 {
                    had_explicit_scroll_adjustment[d] = true;
                }
            }
        }

        // Restore scroll target meant for ScrollAreas up the stack (if any)
        ui.ctx().pass_state_mut(|state| {
            for d in 0..2 {
                if saved_scroll_target[d].is_some() {
                    state.scroll_target[d] = saved_scroll_target[d].clone();
                }
            }
        });

        let inner_rect = {
            // At this point this is the available size for the inner rect.
            let mut inner_size = inner_rect.size();

            for d in 0..2 {
                inner_size[d] = match (direction_enabled[d], auto_shrink[d]) {
                    (true, true) => inner_size[d].min(content_size[d]), // shrink scroll area if content is small
                    (true, false) => inner_size[d], // let scroll area be larger than content; fill with blank space
                    (false, true) => content_size[d], // Follow the content (expand/contract to fit it).
                    (false, false) => inner_size[d].max(content_size[d]), // Expand to fit content
                };
            }

            Rect::from_min_size(inner_rect.min, inner_size)
        };

        let outer_rect = Rect::from_min_size(inner_rect.min, inner_rect.size() + current_bar_use);

        let limit_rect = if ui.spacing().scroll.floating {
            outer_rect
        } else {
            inner_rect
        };

        let content_is_too_large = Vec2b::new(
            direction_enabled[0] && (limit_rect.width().ceil() < content_size.x),
            direction_enabled[1] && (limit_rect.height().ceil() < content_size.y),
        );

        let max_offset = content_size - inner_rect.size();

        // Drag-to-scroll?
        let is_dragging_background = background_drag_response
            .as_ref()
            .is_some_and(|r| r.dragged());

        let is_hovering_outer_rect = ui.rect_contains_pointer(outer_rect)
            && ui.ctx().dragged_id().is_none()
            || is_dragging_background;

        if scroll_source.mouse_wheel
            && ui.is_enabled()
            && (ui.rect_contains_pointer(outer_rect) || is_dragging_background)
        {
            let always_scroll_enabled_direction = ui.style().always_scroll_the_only_direction
                && direction_enabled[0] != direction_enabled[1];
            let scroll_source = ui.input(|i| i.scroll_source());

            for d in 0..2 {
                if direction_enabled[d] && !(skip_native_y && d == 1) {
                    let scroll_delta = ui.input(|input| {
                        if always_scroll_enabled_direction {
                            // no bidirectional scrolling; allow horizontal scrolling without pressing shift
                            input.smooth_scroll_delta()[0] + input.smooth_scroll_delta()[1]
                        } else {
                            input.smooth_scroll_delta()[d]
                        }
                    });
                    let scroll_delta = scroll_delta * wheel_scroll_multiplier[d];

                    let physics = AxisPhysics {
                        bounds: scroll_bounds(max_offset[d]), // We know this frame's bounds now
                        ..axis_physics[d]
                    };

                    let consumed = physics.wheel(
                        &mut state.offset[d],
                        &mut state.vel[d],
                        WheelScroll {
                            // Offset has an inverted coordinate system compared to the scroll delta:
                            delta: -scroll_delta,
                            source: scroll_source,
                            can_chain_to_parent: has_scrolling_ancestor,
                            dt: ui.input(|i| i.stable_dt).at_most(0.1),
                        },
                        &mut state.bounced_this_momentum[d],
                    );

                    if consumed {
                        rubber_banding[d] = physics.is_rubber_banding(state.offset[d]);

                        // Clear scroll delta so no parent scroll will use it:
                        ui.input_mut(|input| {
                            if always_scroll_enabled_direction {
                                input.smooth_scroll_delta = Vec2::ZERO;
                            } else {
                                input.smooth_scroll_delta[d] = 0.0;
                            }
                        });

                        state.scroll_stuck_to_end[d] = false;
                        state.offset_target[d] = None;
                    }
                }
            }
        }

        let show_scroll_this_frame = match scroll_bar_visibility {
            ScrollBarVisibility::AlwaysHidden => Vec2b::FALSE,
            ScrollBarVisibility::VisibleWhenNeeded => content_is_too_large,
            ScrollBarVisibility::AlwaysVisible => direction_enabled,
        };

        // Avoid frame delay; start showing scroll bar right away:
        if show_scroll_this_frame[0] && show_bars_factor.x <= 0.0 {
            show_bars_factor.x = ui.ctx().animate_bool_responsive(id.with("h"), true);
        }
        if show_scroll_this_frame[1] && show_bars_factor.y <= 0.0 {
            show_bars_factor.y = ui.ctx().animate_bool_responsive(id.with("v"), true);
        }

        let scroll_style = ui.spacing().scroll;

        // Reserve the scroll area before painting fades, because fade painting uses ui.min_rect().
        ui.advance_cursor_after_rect(outer_rect);

        paint_fade_areas_impl(ui, inner_rect, content_size, state.offset);

        // Paint the bars:
        let scroll_bar_rect = scroll_bar_rect.unwrap_or(inner_rect);
        for d in 0..2 {
            // maybe force increase in offset to keep scroll stuck to end position,
            // unless this axis had an explicit scroll adjustment.
            if stick_to_end[d] && state.scroll_stuck_to_end[d] && !had_explicit_scroll_adjustment[d]
            {
                state.offset[d] = content_size[d] - inner_rect.size()[d];
            }

            let show_factor = show_bars_factor[d];
            if show_factor == 0.0 {
                state.scroll_bar_interaction[d] = false;
                continue;
            }

            let interact_id = if d == 1 {
                external_vertical_scroll
                    .map_or(id, |external| external.interaction_id)
                    .with(d)
            } else {
                id.with(d)
            };

            // Margin on either side of the scroll bar:
            let inner_margin = show_factor * scroll_style.bar_inner_margin;
            let outer_margin = show_factor * scroll_style.bar_outer_margin;

            // bottom of a horizontal scroll (d==0).
            // right of a vertical scroll (d==1).
            let mut max_cross = outer_rect.max[1 - d] - outer_margin;

            if ui.clip_rect().max[1 - d] - outer_margin < max_cross {
                // Move the scrollbar so it is visible. This is needed in some cases.
                // For instance:
                // * When we have a vertical-only scroll area in a top level panel,
                //   and that panel is not wide enough for the contents.
                // * When one ScrollArea is nested inside another, and the outer
                //   is scrolled so that the scroll-bars of the inner ScrollArea (us)
                //   is outside the clip rectangle.
                max_cross = ui.clip_rect().max[1 - d] - outer_margin;
            }

            let full_width = scroll_style.bar_width;

            // The bounding rect of a fully visible bar.
            // When we hover this area, we should show the full bar.
            // The interaction rect must match where the scroll bar is drawn (outer_scroll_bar_rect),
            // i.e. use `scroll_bar_rect` along the scrolling axis. Otherwise a press in a region
            // where no scrollbar is drawn is sensed as a scrollbar press, creating a dead zone
            // and snapping the offset to the minimum (see issue #8495).
            let max_bar_rect = if d == 0 {
                Rect::from_x_y_ranges(
                    scroll_bar_rect.x_range(),
                    Rangef::new(max_cross - full_width, max_cross),
                )
            } else {
                Rect::from_x_y_ranges(
                    Rangef::new(max_cross - full_width, max_cross),
                    scroll_bar_rect.y_range(),
                )
            };

            let sense = if scroll_source.scroll_bar && ui.is_enabled() {
                Sense::CLICK | Sense::DRAG
            } else {
                Sense::hover()
            };

            // We always sense interaction with the full width, even if we antimate it growing/shrinking.
            // This is to present a more consistent target for our hit test code,
            // and to avoid producing jitter in "thin widget" heuristics there.
            // Also: it make sense to detect any hover where the scroll bar _will_ be.
            let response = ui.interact(max_bar_rect, interact_id, sense);

            response.widget_info(|| WidgetInfo::new(Role::ScrollBar));

            // top/bottom of a horizontal scroll (d==0).
            // left/rigth of a vertical scroll (d==1).
            let cross = if scroll_style.floating {
                let is_hovering_bar_area = response.hovered() || state.scroll_bar_interaction[d];

                let is_hovering_bar_area_t = ui
                    .ctx()
                    .animate_bool_responsive(id.with((d, "bar_hover")), is_hovering_bar_area);

                let width = show_factor
                    * lerp(
                        scroll_style.floating_width..=full_width,
                        is_hovering_bar_area_t,
                    );

                let min_cross = max_cross - width;
                Rangef::new(min_cross, max_cross)
            } else {
                let min_cross = inner_rect.max[1 - d] + inner_margin;
                Rangef::new(min_cross, max_cross)
            };

            let outer_scroll_bar_rect = if d == 0 {
                Rect::from_x_y_ranges(scroll_bar_rect.x_range(), cross)
            } else {
                Rect::from_x_y_ranges(cross, scroll_bar_rect.y_range())
            };

            let from_content = |content| {
                remap_clamp(
                    content,
                    0.0..=content_size[d],
                    scroll_bar_rect.min[d]..=scroll_bar_rect.max[d],
                )
            };

            let calculate_handle_rect = |d, offset: &Vec2| {
                let handle_size = if d == 0 {
                    from_content(offset.x + inner_rect.width()) - from_content(offset.x)
                } else {
                    from_content(offset.y + inner_rect.height()) - from_content(offset.y)
                }
                .max(scroll_style.handle_min_length);

                let handle_start_point = remap_clamp(
                    offset[d],
                    0.0..=max_offset[d],
                    scroll_bar_rect.min[d]..=(scroll_bar_rect.max[d] - handle_size),
                );

                if d == 0 {
                    Rect::from_min_max(
                        pos2(handle_start_point, cross.min),
                        pos2(handle_start_point + handle_size, cross.max),
                    )
                } else {
                    Rect::from_min_max(
                        pos2(cross.min, handle_start_point),
                        pos2(cross.max, handle_start_point + handle_size),
                    )
                }
            };

            let handle_rect = calculate_handle_rect(d, &state.offset);

            state.scroll_bar_interaction[d] = response.hovered() || response.dragged();

            let external_bar_position = if d == 1 {
                external_vertical_scroll.and_then(|external| {
                    if let Some((ExternalCaptureRole::VerticalBar, press)) =
                        external_capture_input.new_press
                        && !(scroll_source.scroll_bar
                            && ui.is_enabled()
                            && external_press_in_rect(ui, max_bar_rect, press.position))
                    {
                        finish_external_bar_press(ui.ctx(), external, press);
                        return None;
                    }
                    if let Some(press) = external_capture_input.short_press
                        && response.clicked_by(press.button)
                        && scroll_source.scroll_bar
                        && ui.is_enabled()
                        && external_press_in_rect(ui, max_bar_rect, press.position)
                    {
                        finish_external_bar_press(ui.ctx(), external, press);
                        state.vel.y = 0.0;
                        state.offset_target[1] = None;
                        state.scroll_start_offset_from_top_left[1] = None;
                        return Some(press.position);
                    }
                    (scroll_source.scroll_bar && ui.is_enabled())
                        .then_some(external_capture_input.bar_position)
                        .flatten()
                })
            } else {
                None
            };
            let pointer_pos = if d == 1 && external_vertical_scroll.is_some() {
                external_bar_position
            } else {
                response.interact_pointer_pos()
            };
            if skip_native_y && d == 1 {
                // Application gestures and replayed input are already applied.
                // Keep the transport and grab anchor without moving it twice.
            } else if let Some(pointer_pos) = pointer_pos {
                let grab_position = if let Some((ExternalCaptureRole::VerticalBar, press)) =
                    external_capture_input.new_press
                {
                    press.position
                } else {
                    pointer_pos
                };
                let scroll_start_offset_from_top_left = state.scroll_start_offset_from_top_left[d]
                    .get_or_insert_with(|| {
                        if handle_rect.contains(grab_position) {
                            grab_position[d] - handle_rect.min[d]
                        } else {
                            let handle_top_pos_at_bottom =
                                scroll_bar_rect.max[d] - handle_rect.size()[d];
                            // Calculate the new handle top position, centering the handle on the mouse.
                            let new_handle_top_pos = (grab_position[d]
                                - handle_rect.size()[d] * 0.5)
                                .clamp(scroll_bar_rect.min[d], handle_top_pos_at_bottom);
                            grab_position[d] - new_handle_top_pos
                        }
                    });

                let new_handle_top = pointer_pos[d] - *scroll_start_offset_from_top_left;
                let handle_travel =
                    scroll_bar_rect.min[d]..=(scroll_bar_rect.max[d] - handle_rect.size()[d]);
                state.offset[d] = if handle_travel.start() == handle_travel.end() {
                    0.0
                } else {
                    remap(new_handle_top, handle_travel, 0.0..=max_offset[d])
                };

                // some manual action taken, scroll not stuck
                state.scroll_stuck_to_end[d] = false;
                state.offset_target[d] = None;
            } else {
                state.scroll_start_offset_from_top_left[d] = None;
            }

            if !rubber_banding[d] {
                let unbounded_offset = state.offset[d];
                state.offset[d] = scroll_bounds(max_offset[d]).clamp(state.offset[d]);

                if state.offset[d] != unbounded_offset {
                    state.vel[d] = 0.0;
                }
            }

            if ui.is_rect_visible(outer_scroll_bar_rect) {
                // Avoid frame-delay by calculating a new handle rect:
                let handle_rect = calculate_handle_rect(d, &state.offset);

                let visuals = if scroll_source.scroll_bar && ui.is_enabled() {
                    // Pick visuals based on interaction with the handle.
                    // Remember that the response is for the whole scroll bar!
                    let is_hovering_handle = response.hovered()
                        && ui.input(|i| {
                            i.pointer
                                .latest_pos()
                                .is_some_and(|p| handle_rect.contains(p))
                        });
                    let visuals = ui.visuals();
                    if response.is_pointer_button_down_on() {
                        &visuals.widgets.active
                    } else if is_hovering_handle {
                        &visuals.widgets.hovered
                    } else {
                        &visuals.widgets.inactive
                    }
                } else {
                    &ui.visuals().widgets.inactive
                };

                let handle_opacity = if scroll_style.floating {
                    if response.hovered() || response.dragged() {
                        scroll_style.interact_handle_opacity
                    } else {
                        let is_hovering_outer_rect_t = ui.ctx().animate_bool_responsive(
                            id.with((d, "is_hovering_outer_rect")),
                            is_hovering_outer_rect,
                        );
                        lerp(
                            scroll_style.dormant_handle_opacity
                                ..=scroll_style.active_handle_opacity,
                            is_hovering_outer_rect_t,
                        )
                    }
                } else {
                    1.0
                };

                let background_opacity = if scroll_style.floating {
                    if response.hovered() || response.dragged() {
                        scroll_style.interact_background_opacity
                    } else if is_hovering_outer_rect {
                        scroll_style.active_background_opacity
                    } else {
                        scroll_style.dormant_background_opacity
                    }
                } else {
                    1.0
                };

                let handle_color = if scroll_style.foreground_color {
                    visuals.fg_stroke.color
                } else {
                    visuals.bg_fill
                };

                // Background:
                ui.painter().add(epaint::Shape::rect_filled(
                    outer_scroll_bar_rect,
                    visuals.corner_radius,
                    ui.visuals()
                        .extreme_bg_color
                        .gamma_multiply(background_opacity),
                ));

                // Handle:
                ui.painter().add(epaint::Shape::rect_filled(
                    handle_rect,
                    visuals.corner_radius,
                    handle_color.gamma_multiply(handle_opacity),
                ));
            }
        }

        if show_scroll_this_frame != state.show_scroll {
            ui.request_repaint();
        }

        let available_offset = content_size - inner_rect.size();
        for d in 0..2 {
            if !rubber_banding[d] {
                state.offset[d] = scroll_bounds(available_offset[d]).clamp(state.offset[d]);
            }
        }
        state.max_offset = available_offset;

        let suppress_stuck_recompute = Vec2b::new(
            had_explicit_scroll_adjustment[0] && state.offset_target[0].is_some(),
            had_explicit_scroll_adjustment[1] && state.offset_target[1].is_some(),
        );

        // Is scroll handle at end of content, or is there no scrollbar
        // yet (not enough content), but sticking is requested? If so, enter sticky mode.
        // Only has an effect if stick_to_end is enabled but we save in
        // state anyway so that entering sticky mode at an arbitrary time
        // has appropriate effect.
        // Keep explicit target requests from being reclassified as "still stuck" in the same
        // frame, otherwise animated scroll-to requests never get a chance to pull away from the end.
        state.scroll_stuck_to_end = Vec2b::new(
            !suppress_stuck_recompute[0]
                && ((state.offset[0] == available_offset[0])
                    || (stick_to_end[0] && available_offset[0] < 0.0)),
            !suppress_stuck_recompute[1]
                && ((state.offset[1] == available_offset[1])
                    || (stick_to_end[1] && available_offset[1] < 0.0)),
        );

        state.show_scroll = show_scroll_this_frame;
        state.content_is_too_large = content_is_too_large;
        state.interact_rect = Some(inner_rect);

        state.store(ui.ctx(), id);

        (content_size, state)
    }
}

/// Paint fade-out gradients at the top and/or bottom of a scroll area to
/// indicate that more content is available beyond the visible region.
fn paint_fade_areas_impl(ui: &Ui, inner_rect: Rect, content_size: Vec2, offset: Vec2) {
    let crate::style::ScrollFadeStyle {
        strength,
        size: fade_size,
    } = ui.spacing().scroll.fade;

    if strength <= 0.0 {
        return;
    }

    let bg = ui.stack().bg_color();

    let overflow = content_size - inner_rect.size();

    let paint_rect = inner_rect.intersect(ui.min_rect());

    // Top fade: animate opacity based on how far we've scrolled down.
    if 0.0 < offset.y {
        let t = (offset.y / fade_size).clamp(0.0, 1.0) * strength;
        let bg_faded = bg.gamma_multiply(t);
        let rect = Rect::from_min_max(
            paint_rect.left_top(),
            pos2(paint_rect.right(), paint_rect.top() + fade_size),
        );
        ui.painter().add(Shape::gradient_rect(
            rect,
            Direction::TopDown,
            [bg_faded, Color32::TRANSPARENT],
        ));
    }

    // Bottom fade: animate opacity based on distance from the bottom.
    let distance_from_bottom = overflow.y - offset.y;
    if 0.0 < distance_from_bottom {
        let t = (distance_from_bottom / fade_size).clamp(0.0, 1.0) * strength;
        let bg_faded = bg.gamma_multiply(t);
        let rect = Rect::from_min_max(
            pos2(paint_rect.left(), paint_rect.bottom() - fade_size),
            paint_rect.right_bottom(),
        );
        ui.painter().add(Shape::gradient_rect(
            rect,
            Direction::BottomUp,
            [bg_faded, Color32::TRANSPARENT],
        ));
    }

    // Left fade: animate opacity based on how far we've scrolled right.
    if 0.0 < offset.x {
        let t = (offset.x / fade_size).clamp(0.0, 1.0) * strength;
        let bg_faded = bg.gamma_multiply(t);
        let rect = Rect::from_min_max(
            paint_rect.left_top(),
            pos2(paint_rect.left() + fade_size, paint_rect.bottom()),
        );
        ui.painter().add(Shape::gradient_rect(
            rect,
            Direction::LeftToRight,
            [bg_faded, Color32::TRANSPARENT],
        ));
    }

    // Right fade: animate opacity based on distance from the right edge.
    let distance_from_right = overflow.x - offset.x;
    if 0.0 < distance_from_right {
        let t = (distance_from_right / fade_size).clamp(0.0, 1.0) * strength;
        let bg_faded = bg.gamma_multiply(t);
        let rect = Rect::from_min_max(
            pos2(paint_rect.right() - fade_size, paint_rect.top()),
            paint_rect.right_bottom(),
        );
        ui.painter().add(Shape::gradient_rect(
            rect,
            Direction::RightToLeft,
            [bg_faded, Color32::TRANSPARENT],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScrollSample {
        id: Id,
        state: State,
        inner_rect: Rect,
        viewport: Rect,
        origin: Pos2,
        content_size: Vec2,
    }

    fn external_frame(
        ctx: &Context,
        content_height: f32,
        viewport_height: f32,
        position: ExternalScrollPosition,
        events: Vec<crate::Event>,
    ) -> ScrollSample {
        sample_frame(ctx, content_height, viewport_height, Some(position), events)
    }

    fn sample_frame(
        ctx: &Context,
        content_height: f32,
        viewport_height: f32,
        position: Option<ExternalScrollPosition>,
        events: Vec<crate::Event>,
    ) -> ScrollSample {
        owner_frame(
            ctx,
            content_height,
            viewport_height,
            position,
            Id::unique("external-test-session"),
            Id::unique("external-test-geometry"),
            events,
        )
    }

    fn owner_frame(
        ctx: &Context,
        content_height: f32,
        viewport_height: f32,
        position: Option<ExternalScrollPosition>,
        session: Id,
        geometry: Id,
        events: Vec<crate::Event>,
    ) -> ScrollSample {
        owner_frame_with_other(
            ctx,
            content_height,
            viewport_height,
            position,
            session,
            geometry,
            events,
            None,
        )
    }

    fn owner_frame_with_other(
        ctx: &Context,
        content_height: f32,
        viewport_height: f32,
        position: Option<ExternalScrollPosition>,
        session: Id,
        geometry: Id,
        events: Vec<crate::Event>,
        other: Option<(Id, Rect)>,
    ) -> ScrollSample {
        let mut sample = None;
        ctx.run_pass(
            crate::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                time: Some((ctx.cumulative_frame_nr() + 1) as f64 / 60.0),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    crate::CentralPanel::default().show(ui, |ui| {
                        ui.spacing_mut().scroll = crate::style::ScrollStyle::solid();
                        let mut area = ScrollArea::vertical()
                            .id_salt(geometry)
                            .auto_shrink([false, false])
                            .min_scrolled_height(64.0)
                            .max_height(viewport_height)
                            .content_margin(Margin::ZERO)
                            .scroll_source(ScrollSource::ALL);
                        if let Some(position) = position {
                            area = area.external_vertical_scroll(ExternalVerticalScroll {
                                content_height,
                                position,
                                interaction_session: session,
                                interaction_id: Id::unique("external-test-transport"),
                            });
                        }
                        let output = area.show_viewport(ui, |ui, viewport| {
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(200.0, content_height),
                                Sense::hover(),
                            );
                            (viewport, rect.min)
                        });
                        sample = Some(ScrollSample {
                            id: output.id,
                            state: output.state,
                            inner_rect: output.inner_rect,
                            viewport: output.inner.0,
                            origin: output.inner.1,
                            content_size: output.content_size,
                        });
                        if let Some((id, rect)) = other {
                            ui.interact(rect, id, Sense::drag());
                        }
                    });
                });
            },
        )
        .drop_without_applying_deltas();
        sample.unwrap()
    }

    fn pointer_button(pos: Pos2, pressed: bool) -> crate::Event {
        crate::Event::PointerButton {
            pos,
            button: crate::PointerButton::Primary,
            pressed,
            modifiers: crate::Modifiers::NONE,
        }
    }

    fn assert_draw_position(sample: &ScrollSample, expected: f32) {
        assert_eq!(sample.viewport.min.y, expected);
        assert!(
            (sample.origin.y - (sample.inner_rect.min.y - expected)).abs() <= 1.0,
            "content origin {:?}, viewport {:?}, offset {expected}",
            sample.origin,
            sample.inner_rect,
        );
    }

    #[test]
    fn external_vertical_current_geometry_precedes_physics_and_first_draw() {
        let ctx = Context::default();
        let old = external_frame(&ctx, 400.0, 200.0, ExternalScrollPosition::Set(0.0), vec![]);
        assert_eq!(old.state.max_offset.y, 200.0);
        let grown = external_frame(
            &ctx,
            1000.0,
            200.0,
            ExternalScrollPosition::Set(800.0),
            vec![],
        );
        assert_eq!(grown.state.offset.y, 800.0);
        assert_eq!(grown.state.max_offset.y, 800.0);
        assert_draw_position(&grown, 800.0);
        let shrunk = external_frame(
            &ctx,
            600.0,
            300.0,
            ExternalScrollPosition::Set(300.0),
            vec![],
        );
        assert_eq!(shrunk.state.max_offset.y, 300.0);
        assert_eq!(shrunk.state.offset.y, 300.0);
        assert_draw_position(&shrunk, 300.0);
        let short = external_frame(&ctx, 20.0, 10.0, ExternalScrollPosition::Set(0.0), vec![]);
        assert_eq!(short.inner_rect.height(), 64.0);
        assert_eq!(short.content_size.y, 20.0);
        assert!(!short.state.content_is_too_large.y);
        assert_draw_position(&short, 0.0);
    }

    #[test]
    fn external_vertical_set_ends_velocity_and_target_before_drawing() {
        for velocity in [0.0, 900.0] {
            for target in [false, true] {
                for (height, viewport) in [(760.0, 200.0), (1000.0, 440.0)] {
                    let ctx = Context::default();
                    let first = external_frame(
                        &ctx,
                        1200.0,
                        200.0,
                        ExternalScrollPosition::Set(840.0),
                        vec![],
                    );
                    let mut state = first.state;
                    state.vel.y = velocity;
                    state.offset_target[1] = target.then_some(ScrollingToTarget {
                        animation_time_span: (0.0, 1.0),
                        target_offset: 950.0,
                    });
                    state.store(&ctx, first.id);
                    let set = external_frame(
                        &ctx,
                        height,
                        viewport,
                        ExternalScrollPosition::Set(560.0),
                        vec![],
                    );
                    assert_eq!(set.state.offset.y, 560.0);
                    assert_eq!(set.state.vel.y, 0.0);
                    assert!(set.state.offset_target[1].is_none());
                    assert_draw_position(&set, 560.0);
                    let next = external_frame(
                        &ctx,
                        height,
                        viewport,
                        ExternalScrollPosition::Sync(560.0),
                        vec![],
                    );
                    assert_eq!(next.state.offset.y, 560.0);
                }
            }
        }
    }

    #[test]
    fn external_vertical_sync_keeps_motion_and_draws_its_result() {
        for animation in [false, true] {
            let ctx = Context::default();
            let first = external_frame(
                &ctx,
                1200.0,
                200.0,
                ExternalScrollPosition::Set(200.0),
                vec![],
            );
            let mut state = first.state;
            state.vel.y = if animation { 0.0 } else { 500.0 };
            state.offset_target[1] = animation.then_some(ScrollingToTarget {
                animation_time_span: (0.0, 1.0),
                target_offset: 800.0,
            });
            state.store(&ctx, first.id);
            let sync = external_frame(
                &ctx,
                1200.0,
                200.0,
                ExternalScrollPosition::Sync(200.0),
                vec![],
            );
            assert!(sync.state.offset.y > 200.0);
            if animation {
                assert!(sync.state.offset_target[1].is_some());
            } else {
                assert!(sync.state.vel.y > 0.0);
            }
            assert_draw_position(&sync, sync.state.offset.y);
        }
    }

    #[test]
    fn external_vertical_out_of_range_sync_terminates_old_motion() {
        let ctx = Context::default();
        let first = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(840.0),
            vec![],
        );
        let mut state = first.state;
        state.vel.y = 500.0;
        state.offset_target[1] = Some(ScrollingToTarget {
            animation_time_span: (0.0, 1.0),
            target_offset: 950.0,
        });
        state.store(&ctx, first.id);
        let sync = external_frame(
            &ctx,
            760.0,
            200.0,
            ExternalScrollPosition::Sync(840.0),
            vec![],
        );
        assert_eq!(sync.state.offset.y, 560.0);
        assert_eq!(sync.state.vel.y, 0.0);
        assert!(sync.state.offset_target[1].is_none());
        assert_draw_position(&sync, 560.0);
    }

    #[test]
    fn external_vertical_app_gesture_does_not_apply_background_drag_twice() {
        let ctx = Context::default();
        let first = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(300.0),
            vec![],
        );
        let start = first.inner_rect.center();
        // Stage B retains the native previous-frame interaction rectangle.
        external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(300.0),
            vec![],
        );
        external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(300.0),
            vec![pointer_button(start, true)],
        );
        let moved = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::AppGesture(337.5),
            vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 40.0))],
        );
        assert_eq!(
            ctx.memory(|m| m.interaction().potential_drag_id),
            Some(Id::unique("external-test-transport").with("area"))
        );
        assert_eq!(moved.state.offset.y, 337.5);
        assert_eq!(moved.state.vel.y, 0.0);
        assert_draw_position(&moved, 337.5);
        let released = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::AppGesture(350.0),
            vec![pointer_button(start - Vec2::new(0.0, 40.0), false)],
        );
        assert_eq!(released.state.offset.y, 350.0);
        assert_eq!(released.state.vel.y, 0.0);
    }

    #[test]
    fn external_vertical_set_preserves_held_bar_and_app_gesture_owns_it() {
        let ctx = Context::default();
        external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(0.0),
            vec![],
        );
        let first = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(0.0),
            vec![],
        );
        let bar = pos2(first.inner_rect.right() + 6.0, first.inner_rect.top() + 8.0);
        let held = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(0.0),
            vec![pointer_button(bar, true)],
        );
        assert_eq!(
            ctx.memory(|m| m.interaction().potential_drag_id),
            Some(Id::unique("external-test-transport").with(1_usize))
        );
        let anchor =
            held.state.scroll_start_offset_from_top_left[1].unwrap_or_else(|| {
                panic!(
                    "bar is held: first={:?}, held={:?}, pointer={bar:?}, capture={:?}",
                    first.inner_rect,
                    held.inner_rect,
                    ctx.data(|d| d.get_temp::<ExternalScrollCapture>(external_capture_id(
                        Id::unique("external-test-transport")
                    )))
                )
            });
        let moved = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(200.0),
            vec![crate::Event::PointerMoved(bar + Vec2::new(0.0, 40.0))],
        );
        assert_eq!(
            moved.state.scroll_start_offset_from_top_left[1],
            Some(anchor)
        );
        assert!(
            moved.state.offset.y > 200.0,
            "current bar input must follow Set"
        );
        let app = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::AppGesture(237.5),
            vec![crate::Event::PointerMoved(bar + Vec2::new(0.0, 60.0))],
        );
        assert_eq!(app.state.offset.y, 237.5);
    }

    #[test]
    fn native_scroll_keeps_pre_physics_placement_and_coast() {
        let ctx = Context::default();
        let first = sample_frame(&ctx, 1200.0, 200.0, None, vec![]);
        let mut state = first.state;
        state.offset.y = 200.0;
        state.vel.y = 500.0;
        state.store(&ctx, first.id);
        let native = sample_frame(&ctx, 1200.0, 200.0, None, vec![]);
        assert!(native.state.offset.y > 200.0);
        assert!(native.state.vel.y > 0.0);
        assert_draw_position(&native, 200.0);
    }

    #[test]
    fn external_vertical_equal_value_set_still_ends_previous_motion() {
        let ctx = Context::default();
        let first = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(200.0),
            vec![],
        );
        let mut state = first.state;
        state.vel.y = 500.0;
        state.offset_target[1] = Some(ScrollingToTarget {
            animation_time_span: (0.0, 1.0),
            target_offset: 800.0,
        });
        state.store(&ctx, first.id);
        let set = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Set(200.0),
            vec![],
        );
        assert_eq!(set.state.offset.y, 200.0);
        assert_eq!(set.state.vel.y, 0.0);
        assert!(set.state.offset_target[1].is_none());
        assert_draw_position(&set, 200.0);
    }

    #[test]
    fn native_nested_both_axes_stick_and_scroll_to_controls() {
        let ctx = Context::default();
        for pass in 0..3 {
            ctx.run_pass(crate::RawInput::default(), |ctx| {
                ctx.root_ui(|ui| {
                    crate::CentralPanel::default().show(ui, |ui| {
                        let outer = ScrollArea::both()
                            .max_width(200.0)
                            .max_height(200.0)
                            .auto_shrink([false, false])
                            .scroll_offset(Vec2::new(40.0, 50.0))
                            .animated(false)
                            .show(ui, |ui| {
                                ui.set_min_size(Vec2::splat(600.0));
                                let inner = ScrollArea::vertical()
                                    .id_salt("nested-native")
                                    .max_height(100.0)
                                    .auto_shrink([false, false])
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        ui.allocate_exact_size(
                                            Vec2::new(100.0, 300.0),
                                            Sense::hover(),
                                        );
                                    });
                                assert_eq!(inner.state.offset.y, 200.0);
                                if pass == 2 {
                                    ui.scroll_to_rect_animation(
                                        Rect::from_min_size(
                                            ui.min_rect().min + Vec2::splat(350.0),
                                            Vec2::splat(20.0),
                                        ),
                                        Some(crate::Align::Center),
                                        crate::style::ScrollAnimation::none(),
                                    );
                                }
                            });
                        assert!(outer.state.max_offset.x > 0.0);
                        assert!(outer.state.max_offset.y > 0.0);
                        if pass == 1 {
                            assert_eq!(outer.state.offset, Vec2::new(40.0, 50.0));
                        } else if pass == 2 {
                            assert!(outer.state.offset.x > 200.0);
                            assert!(outer.state.offset.y > 200.0);
                        }
                    });
                });
            })
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn native_nested_short_content_chains_wheel_to_parent() {
        let ctx = Context::default();
        let mut pointer = Pos2::ZERO;
        for pass in 0..3 {
            let events = if pass == 2 {
                vec![
                    crate::Event::PointerMoved(pointer),
                    crate::Event::MouseWheel {
                        unit: crate::MouseWheelUnit::Point,
                        delta: Vec2::new(0.0, -60.0),
                        phase: crate::TouchPhase::Move,
                        source: MouseWheelSource::Unknown,
                        modifiers: crate::Modifiers::NONE,
                    },
                ]
            } else {
                vec![]
            };
            ctx.run_pass(
                crate::RawInput {
                    time: Some((pass + 1) as f64 / 60.0),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        crate::CentralPanel::default().show(ui, |ui| {
                            let outer = ScrollArea::vertical()
                                .max_height(200.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_height(1000.0);
                                    let inner = ScrollArea::vertical()
                                        .id_salt("short-nested-native")
                                        .max_height(100.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.allocate_exact_size(
                                                Vec2::new(100.0, 20.0),
                                                Sense::hover(),
                                            );
                                        });
                                    pointer = inner.inner_rect.center();
                                    assert_eq!(inner.state.offset.y, 0.0);
                                });
                            if pass == 2 {
                                assert!(outer.state.offset.y > 0.0);
                                assert_eq!(ctx.input(|input| input.smooth_scroll_delta.y), 0.0);
                            }
                        });
                    });
                },
            )
            .drop_without_applying_deltas();
        }
    }

    fn capture(ctx: &Context) -> ExternalScrollCapture {
        ctx.data(|data| {
            data.get_temp::<ExternalScrollCapture>(external_capture_id(Id::unique(
                "external-test-transport",
            )))
        })
        .expect("transport was observed")
    }

    #[test]
    fn external_capture_old_background_release_then_new_press_survives_response_aggregation() {
        for (height, viewport) in [(1200.0, 200.0), (2100.0, 240.0)] {
            for switch_geometry in [false, true] {
                let ctx = Context::default();
                let a = Id::unique("owner-a");
                let b = Id::unique("owner-b");
                let geometry_a = Id::unique("geometry-a");
                let geometry_b = if switch_geometry {
                    Id::unique("geometry-b")
                } else {
                    geometry_a
                };
                let first = owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(300.0)),
                    a,
                    geometry_a,
                    vec![],
                );
                let start = first.inner_rect.center();
                owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(300.0)),
                    a,
                    geometry_a,
                    vec![pointer_button(start, true)],
                );
                let dragged = owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(300.0)),
                    a,
                    geometry_a,
                    vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 30.0))],
                );
                assert_eq!(dragged.state.offset.y, 330.0);
                let stamp_frame = ctx.cumulative_frame_nr();
                let switched = owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(500.0)),
                    b,
                    geometry_b,
                    vec![
                        pointer_button(start - Vec2::new(0.0, 30.0), false),
                        pointer_button(start, true),
                    ],
                );
                let response = ctx
                    .read_response(Id::unique("external-test-transport").with("area"))
                    .unwrap();
                assert!(
                    !response.dragged() && !response.drag_started() && !response.drag_stopped()
                );
                assert!(!response.is_pointer_button_down_on() && !response.clicked());
                assert_eq!(
                    ctx.memory(|m| m.interaction().potential_drag_id),
                    Some(Id::unique("external-test-transport").with("area"))
                );
                assert!(matches!(capture(&ctx), ExternalScrollCapture::Sequence {
                    press_stamp: ExternalPressStamp { frame, ordinal: 0 }, session, role: ExternalCaptureRole::Background,
                    phase: ExternalCapturePhase::Held { .. }, ..
                } if frame == stamp_frame && session == b));
                assert_eq!(switched.state.offset.y, 500.0);
                assert_eq!(switched.state.vel.y, 0.0);
                let moved = owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(500.0)),
                    b,
                    geometry_b,
                    vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 40.0))],
                );
                assert_eq!(moved.state.offset.y, 540.0);
                let released = owner_frame(
                    &ctx,
                    height,
                    viewport,
                    Some(ExternalScrollPosition::Sync(540.0)),
                    b,
                    geometry_b,
                    vec![pointer_button(start - Vec2::new(0.0, 40.0), false)],
                );
                assert!(released.state.vel.y > 0.0);
            }
        }
    }

    #[test]
    fn external_capture_old_held_and_added_button_do_not_acquire_destination() {
        let ctx = Context::default();
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry");
        let first = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
        );
        let start = first.inner_rect.center();
        owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![pointer_button(start, true)],
        );
        let mut added = pointer_button(start, true);
        if let crate::Event::PointerButton { button, .. } = &mut added {
            *button = crate::PointerButton::Secondary;
        }
        let switched = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![
                added,
                crate::Event::PointerMoved(start - Vec2::new(0.0, 50.0)),
            ],
        );
        assert_eq!(switched.state.offset.y, 500.0);
        assert_eq!(switched.state.vel.y, 0.0);
        assert!(
            matches!(capture(&ctx), ExternalScrollCapture::Sequence { session, .. } if session == a)
        );
        let released = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![pointer_button(start, false)],
        );
        assert_eq!(released.state.offset.y, 500.0);
        assert_eq!(released.state.vel.y, 0.0);
    }

    #[test]
    fn external_capture_bar_repress_uses_new_session_and_new_grab_anchor() {
        for (height, viewport) in [(1200.0, 200.0), (2100.0, 240.0)] {
            let ctx = Context::default();
            let a = Id::unique("owner-a");
            let b = Id::unique("owner-b");
            let geometry_a = Id::unique("geometry-a");
            let geometry_b = Id::unique("geometry-b");
            owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(0.0)),
                a,
                geometry_a,
                vec![],
            );
            let first = owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(0.0)),
                a,
                geometry_a,
                vec![],
            );
            let start = pos2(first.inner_rect.right() + 3.0, first.inner_rect.top() + 8.0);
            owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(0.0)),
                a,
                geometry_a,
                vec![pointer_button(start, true)],
            );
            let old = owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(0.0)),
                a,
                geometry_a,
                vec![crate::Event::PointerMoved(start + Vec2::new(0.0, 30.0))],
            );
            assert!(old.state.offset.y > 0.0);
            let switched = owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(0.0)),
                b,
                geometry_b,
                vec![
                    pointer_button(start + Vec2::new(0.0, 30.0), false),
                    pointer_button(start, true),
                ],
            );
            let response = ctx
                .read_response(Id::unique("external-test-transport").with(1_usize))
                .unwrap();
            assert!(!response.is_pointer_button_down_on() && !response.dragged());
            assert!(switched.state.scroll_start_offset_from_top_left[1].is_some());
            assert!(
                matches!(capture(&ctx), ExternalScrollCapture::Sequence { session, role: ExternalCaptureRole::VerticalBar, phase: ExternalCapturePhase::Held { .. }, .. } if session == b)
            );
            let moved = owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(switched.state.offset.y)),
                b,
                geometry_b,
                vec![crate::Event::PointerMoved(start + Vec2::new(0.0, 45.0))],
            );
            assert!(moved.state.offset.y > switched.state.offset.y + 100.0);
            let released = owner_frame(
                &ctx,
                height,
                viewport,
                Some(ExternalScrollPosition::Sync(moved.state.offset.y)),
                b,
                geometry_b,
                vec![pointer_button(start + Vec2::new(0.0, 45.0), false)],
            );
            assert_eq!(released.state.vel.y, 0.0);
            assert!(matches!(
                capture(&ctx),
                ExternalScrollCapture::Sequence {
                    phase: ExternalCapturePhase::Released { .. },
                    ..
                }
            ));
        }
    }

    #[test]
    fn external_capture_session_ends_only_own_motion_and_preserves_other_geometry() {
        let ctx = Context::default();
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry");
        let first = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
        );
        let mut state = first.state;
        state.vel.y = 700.0;
        state.offset_target[1] = Some(ScrollingToTarget {
            animation_time_span: (0.0, 1.0),
            target_offset: 900.0,
        });
        state.scroll_start_offset_from_top_left[1] = Some(12.0);
        state.bounced_this_momentum.y = true;
        state.store(&ctx, first.id);
        let sibling_id = Id::unique("sibling-geometry");
        state.store(&ctx, sibling_id);
        let switched = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            b,
            geometry,
            vec![],
        );
        assert_eq!(switched.state.offset.y, 300.0);
        assert_eq!(switched.state.vel.y, 0.0);
        assert!(switched.state.offset_target[1].is_none());
        assert!(switched.state.scroll_start_offset_from_top_left[1].is_none());
        assert!(!switched.state.bounced_this_momentum.y);
        let sibling = State::load(&ctx, sibling_id).unwrap();
        assert_eq!(sibling.vel.y, 700.0);
        assert_eq!(sibling.offset_target[1].unwrap().target_offset, 900.0);
        assert!(sibling.bounced_this_momentum.y);
    }

    #[test]
    fn external_capture_discard_does_not_move_twice_or_reassign_press() {
        let ctx = Context::default();
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry-a");
        let first = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
        );
        let start = first.inner_rect.center();
        let frame = ctx.cumulative_frame_nr();
        let mut canonical = 300.0;
        let mut observations = Vec::new();
        ctx.run_pass(
            crate::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                time: Some((frame + 1) as f64 / 60.0),
                events: vec![
                    pointer_button(start, true),
                    crate::Event::PointerMoved(start - Vec2::new(0.0, 40.0)),
                ],
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    crate::CentralPanel::default().show(ui, |ui| {
                        ui.spacing_mut().scroll = crate::style::ScrollStyle::solid();
                        let session = if ctx.current_pass_index() == 0 { a } else { b };
                        let output = ScrollArea::vertical()
                            .id_salt(geometry)
                            .auto_shrink([false, false])
                            .min_scrolled_height(64.0)
                            .max_height(200.0)
                            .scroll_source(ScrollSource::ALL)
                            .content_margin(Margin::ZERO)
                            .external_vertical_scroll(ExternalVerticalScroll {
                                content_height: 1200.0,
                                position: ExternalScrollPosition::Sync(canonical),
                                interaction_session: session,
                                interaction_id: Id::unique("external-test-transport"),
                            })
                            .show(ui, |ui| {
                                ui.allocate_exact_size(Vec2::new(200.0, 1200.0), Sense::hover());
                            });
                        canonical = output.state.offset.y;
                        observations.push((ctx.cumulative_frame_nr(), canonical));
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("external capture replay fixture");
                        }
                    });
                });
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(observations, vec![(frame, 340.0), (frame, 340.0)]);
        assert!(matches!(capture(&ctx), ExternalScrollCapture::Sequence {
            press_stamp: ExternalPressStamp { frame: stamp_frame, ordinal: 0 }, session, ..
        } if stamp_frame == frame && session == a));
        let old_move = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(canonical)),
            b,
            geometry,
            vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 70.0))],
        );
        assert_eq!(old_move.state.offset.y, 340.0);
    }

    #[test]
    fn external_sync_discard_keeps_subrow_drag_coast_and_target_before_paint() {
        // mIV deliberately does not adopt less than half a row. A replay must
        // retain native motion even though canonical remains at 280, rather
        // than adopting State.offset without App's row quantization.
        for motion in [0, 1, 2] {
            for replay_kind in [0, 1, 2, 3] {
                let ctx = Context::default();
                let session = Id::unique("subrow-owner");
                let geometry = Id::unique("subrow-geometry");
                let transport = Id::unique("external-test-transport");
                let initial = owner_frame(
                    &ctx,
                    1200.0,
                    200.0,
                    Some(ExternalScrollPosition::Sync(280.0)),
                    session,
                    geometry,
                    vec![],
                );
                let start = initial.inner_rect.center();
                if motion == 0 {
                    owner_frame(
                        &ctx,
                        1200.0,
                        200.0,
                        Some(ExternalScrollPosition::Sync(280.0)),
                        session,
                        geometry,
                        vec![pointer_button(start, true)],
                    );
                } else {
                    let mut state = State::load(&ctx, initial.id).unwrap();
                    if motion == 1 {
                        state.vel.y = 300.0;
                    } else {
                        let now = ctx.input(|input| input.time);
                        state.offset_target[1] = Some(ScrollingToTarget {
                            animation_time_span: (now - 0.5, now + 0.5),
                            target_offset: 320.0,
                        });
                    }
                    state.store(&ctx, initial.id);
                }
                let frame = ctx.cumulative_frame_nr();
                let mut canonical = 280.0;
                let mut observations = Vec::new();
                ctx.run_pass(
                    crate::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                        time: Some((frame + 1) as f64 / 60.0),
                        events: if motion == 0 {
                            vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 10.0))]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.root_ui(|ui| {
                            crate::CentralPanel::default().show(ui, |ui| {
                                ui.spacing_mut().scroll = crate::style::ScrollStyle::solid();
                                let replay = ctx.current_pass_index() != 0;
                                let position = if replay {
                                    match replay_kind {
                                        1 => ExternalScrollPosition::Set(196.0),
                                        2 => ExternalScrollPosition::AppGesture(285.5),
                                        3 => ExternalScrollPosition::Sync(140.0),
                                        _ => ExternalScrollPosition::Sync(canonical),
                                    }
                                } else {
                                    ExternalScrollPosition::Sync(canonical)
                                };
                                let mut viewport = Rect::NOTHING;
                                let mut origin = Pos2::ZERO;
                                let output = ScrollArea::vertical()
                                    .id_salt(geometry)
                                    .auto_shrink([false, false])
                                    .min_scrolled_height(64.0)
                                    .max_height(200.0)
                                    .scroll_source(ScrollSource::ALL)
                                    .content_margin(Margin::ZERO)
                                    .external_vertical_scroll(ExternalVerticalScroll {
                                        content_height: 1200.0,
                                        position,
                                        interaction_session: if replay && replay_kind == 3 {
                                            session.with("new")
                                        } else {
                                            session
                                        },
                                        interaction_id: transport,
                                    })
                                    .show_viewport(ui, |ui, actual_viewport| {
                                        viewport = actual_viewport;
                                        origin = ui.max_rect().min;
                                        ui.allocate_exact_size(
                                            Vec2::new(200.0, 1200.0),
                                            Sense::hover(),
                                        );
                                    });
                                if (output.state.clamped_offset().y - canonical).abs() > 14.0 {
                                    canonical =
                                        (output.state.clamped_offset().y / 28.0).round() * 28.0;
                                }
                                let expected_origin = pos2(
                                    output.inner_rect.left(),
                                    output.inner_rect.top() - viewport.min.y,
                                )
                                .round_to_pixels(ctx.pixels_per_point())
                                .round_ui();
                                assert_eq!(origin.y, expected_origin.y);
                                observations.push((output.state, viewport.min.y));
                                if !replay {
                                    ctx.request_discard("subrow native motion");
                                }
                            });
                        });
                    },
                )
                .drop_without_applying_deltas();
                assert_eq!(observations.len(), 2);
                let first = observations[0];
                let second = observations[1];
                assert!(
                    first.0.offset.y > 280.0 && first.0.offset.y < 294.0,
                    "must establish sub-half-row motion: {first:?}"
                );
                let expected = match replay_kind {
                    1 => 196.0,
                    2 => 285.5,
                    3 => 140.0,
                    _ => first.0.offset.y,
                };
                assert_eq!(
                    second.0.offset.y, expected,
                    "motion={motion} replay_kind={replay_kind}"
                );
                assert_eq!(
                    second.1, expected,
                    "the final content must use the retained or explicit position"
                );
                if replay_kind == 0 {
                    assert_eq!(second.0.vel.y, first.0.vel.y);
                    assert_eq!(
                        second.0.offset_target[1].is_some(),
                        first.0.offset_target[1].is_some()
                    );
                    assert_eq!(canonical, 280.0);
                } else {
                    assert_eq!(second.0.vel.y, 0.0);
                    assert!(second.0.offset_target[1].is_none());
                }
            }
        }
    }

    #[test]
    fn external_sync_discard_keeps_rubber_band_but_resolves_changed_extent() {
        for shrink in [false, true] {
            let ctx = Context::default();
            let session = Id::unique("rubber-owner");
            let geometry = Id::unique("rubber-geometry");
            let first = owner_frame(
                &ctx,
                1200.0,
                200.0,
                Some(ExternalScrollPosition::Sync(1000.0)),
                session,
                geometry,
                vec![],
            );
            let start = first.inner_rect.center();
            owner_frame(
                &ctx,
                1200.0,
                200.0,
                Some(ExternalScrollPosition::Sync(1000.0)),
                session,
                geometry,
                vec![pointer_button(start, true)],
            );
            let frame = ctx.cumulative_frame_nr();
            let mut offsets = Vec::new();
            ctx.run_pass(
                crate::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
                    time: Some((frame + 1) as f64 / 60.0),
                    events: vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 10.0))],
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        crate::CentralPanel::default().show(ui, |ui| {
                            ui.spacing_mut().scroll = crate::style::ScrollStyle::solid();
                            let replay = ctx.current_pass_index() != 0;
                            let mut viewport_y = 0.0;
                            let output = ScrollArea::vertical()
                                .id_salt(geometry)
                                .auto_shrink([false, false])
                                .max_height(200.0)
                                .content_margin(Margin::ZERO)
                                .scroll_source(ScrollSource::ALL)
                                .external_vertical_scroll(ExternalVerticalScroll {
                                    content_height: if replay && shrink { 1000.0 } else { 1200.0 },
                                    position: ExternalScrollPosition::Sync(1000.0),
                                    interaction_id: Id::unique("external-test-transport"),
                                    interaction_session: session,
                                })
                                .show_viewport(ui, |ui, viewport| {
                                    viewport_y = viewport.min.y;
                                    ui.allocate_exact_size(
                                        Vec2::new(
                                            200.0,
                                            if replay && shrink { 1000.0 } else { 1200.0 },
                                        ),
                                        Sense::hover(),
                                    );
                                });
                            offsets.push((output.state.offset.y, viewport_y, output.state.vel.y));
                            if !replay {
                                ctx.request_discard("rubber-band retry");
                            }
                        });
                    });
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(offsets.len(), 2);
            assert!(
                offsets[0].0 > 1000.0,
                "must establish a real native overscroll"
            );
            if shrink {
                assert_eq!(offsets[1], (800.0, 800.0, 0.0));
            } else {
                assert_eq!(
                    offsets[1], offsets[0],
                    "replay must preserve both drawn and stored rubber-band result"
                );
            }
        }
    }

    #[test]
    fn external_capture_does_not_take_press_delivered_to_another_widget() {
        let ctx = Context::default();
        let other = Id::unique("other-widget");
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry");
        owner_frame_with_other(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
            Some((
                other,
                Rect::from_min_size(pos2(40.0, 40.0), Vec2::splat(100.0)),
            )),
        );
        owner_frame_with_other(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
            Some((
                other,
                Rect::from_min_size(pos2(40.0, 40.0), Vec2::splat(100.0)),
            )),
        );
        let press = pos2(70.0, 70.0);
        let switched = owner_frame_with_other(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![pointer_button(press, true)],
            Some((
                other,
                Rect::from_min_size(pos2(40.0, 40.0), Vec2::splat(100.0)),
            )),
        );
        assert_eq!(
            ctx.memory(|m| m.interaction().potential_drag_id),
            Some(other)
        );
        assert_eq!(switched.state.offset.y, 500.0);
        assert!(matches!(capture(&ctx), ExternalScrollCapture::Idle { .. }));
        let moved = owner_frame_with_other(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![crate::Event::PointerMoved(press - Vec2::new(0.0, 30.0))],
            Some((
                other,
                Rect::from_min_size(pos2(40.0, 40.0), Vec2::splat(100.0)),
            )),
        );
        assert_eq!(moved.state.offset.y, 500.0);
    }

    #[test]
    fn external_capture_short_bar_click_retains_native_click_evidence() {
        let ctx = Context::default();
        external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(0.0),
            vec![],
        );
        let first = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(0.0),
            vec![],
        );
        let press = pos2(
            first.inner_rect.right() + 3.0,
            first.inner_rect.top() + 130.0,
        );
        let clicked = external_frame(
            &ctx,
            1200.0,
            200.0,
            ExternalScrollPosition::Sync(0.0),
            vec![pointer_button(press, true), pointer_button(press, false)],
        );
        assert!(
            ctx.read_response(Id::unique("external-test-transport").with(1_usize))
                .unwrap()
                .clicked()
        );
        assert!(clicked.state.offset.y > 500.0);
        assert!(matches!(
            capture(&ctx),
            ExternalScrollCapture::Sequence {
                phase: ExternalCapturePhase::Released { .. },
                ..
            }
        ));
    }

    #[test]
    fn external_capture_first_destination_press_keeps_normal_drag_release_and_coast() {
        let ctx = Context::default();
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry");
        let first = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
        );
        let start = first.inner_rect.center();
        let pressed = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![pointer_button(start, true)],
        );
        assert_eq!(pressed.state.offset.y, 500.0);
        assert!(
            matches!(capture(&ctx), ExternalScrollCapture::Sequence { session, .. } if session == b)
        );
        let moved = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 40.0))],
        );
        assert_eq!(moved.state.offset.y, 540.0);
        let released = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(540.0)),
            b,
            geometry,
            vec![pointer_button(start - Vec2::new(0.0, 40.0), false)],
        );
        assert!(released.state.vel.y > 0.0);
        let coast = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(released.state.offset.y)),
            b,
            geometry,
            vec![],
        );
        assert!(coast.state.offset.y > released.state.offset.y);
    }

    #[test]
    fn external_capture_duplicate_press_and_escape_do_not_revive_old_hold() {
        let ctx = Context::default();
        let a = Id::unique("owner-a");
        let b = Id::unique("owner-b");
        let geometry = Id::unique("geometry");
        let first = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![],
        );
        let start = first.inner_rect.center();
        owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(300.0)),
            a,
            geometry,
            vec![pointer_button(start, true)],
        );
        let repeated = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![pointer_button(start, true)],
        );
        assert_eq!(repeated.state.offset.y, 500.0);
        assert!(
            matches!(capture(&ctx), ExternalScrollCapture::Sequence { session, .. } if session == a)
        );
        owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            b,
            geometry,
            vec![crate::Event::Key {
                key: crate::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: crate::Modifiers::NONE,
            }],
        );
        assert!(matches!(
            capture(&ctx),
            ExternalScrollCapture::Sequence {
                phase: ExternalCapturePhase::Released { .. },
                ..
            }
        ));
        let moved = owner_frame(
            &ctx,
            1200.0,
            200.0,
            Some(ExternalScrollPosition::Sync(500.0)),
            a,
            geometry,
            vec![crate::Event::PointerMoved(start - Vec2::new(0.0, 80.0))],
        );
        assert_eq!(moved.state.offset.y, 500.0);
    }

    #[test]
    fn clamped_offset() {
        let state = State {
            offset: Vec2::new(-10.0, 150.0),
            max_offset: Vec2::new(-20.0, 100.0),
            ..Default::default()
        };
        assert_eq!(state.clamped_offset(), Vec2::new(0.0, 100.0));
        assert_eq!(state.overscroll(), Vec2::new(-10.0, 50.0));
    }
}
