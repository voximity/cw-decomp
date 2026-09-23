//! `plasma::Widget` and its subclasses (Cube.exe), plus the engine-side input routing
//! (hover, capture, focus, pop-up) that drives them, and `cube::GameController::onResize`.
//!
//! Tier B (behaviourally equivalent). Every block cites its Cube.exe address. The
//! original keeps widgets inside a `plasma::Node` scene graph; this port keeps the same
//! shape as two arenas ([`Gui::nodes`], [`Gui::widgets`]) indexed by [`NodeId`] and
//! [`WidgetId`]. Signals the original sends through `Widget::MemberFunctionConnection`
//! (the event map at `Widget+0x150`) and node state changes (`Node::setState`,
//! `0x00636810`) are queued as [`UiEvent`]s for the caller to route.
//!
//! # Geometry model (simplification, see the report)
//!
//! The original multiplies full 4x4 matrices (the node's evaluated `Transformation`,
//! `+0x1b0`, the widget's bind matrix `+0xe8`, the engine root matrix `+0x1f0`). This
//! port uses the 2D affine part ([`glam::Affine2`]) of those matrices: a node's local
//! transform is [`evaluate_transformation`] (`Transformation::evaluate` 0x00678600:
//! translation, pivot, rotation about x/y/z, deformation) of the node's current
//! Transformation values; the widget code writes the translation key as the original
//! does. Perspective terms (a rotation about x or y) are dropped by the affine
//! projection; the shipped files rotate only about z.
//!
//! # Widget field map (`plasma::Widget`, 0x160 bytes)
//!
//! | Offset | Field | plx tag |
//! |---|---|---|
//! | +0x38 | [`Widget::class_id`] (0 widget, 1 button, 2 edit, 4 list) | – |
//! | +0x3c | [`Widget::clip_hit`] (tentative) | – |
//! | +0x48 / +0x50 | [`Widget::inner_bind_pos`] / [`Widget::inner_bind_size`] | `Widget.innerBindPos/innerBindSize` |
//! | +0x58 / +0x60 | [`Widget::bind_pos`] / [`Widget::bind_size`] | `Widget.bindPos/bindSize` |
//! | +0x68 / +0x70 | [`Widget::frame_pos`] / [`Widget::frame_size`] | `Widget.framePos/frameSize` |
//! | +0x78 | [`Widget::min_frame_size`] = bindSize − innerBindSize (set by `update`) | – |
//! | +0x80 | [`Widget::caption`] | `Widget.caption` |
//! | +0x98 / +0xa0 | drag start cursor / translation | – |
//! | +0xa8 / +0xe8 | `Widget.bindMatrix` / its inverse, [`Widget::bind_matrix`] (0x00687ad0) | `Widget.bindMatrix` |
//! | +0x128 | [`Widget::flags`] ([`flags`]) | `Widget.flags` |
//! | +0x12c / +0x130 | [`Widget::v_align`] / [`Widget::h_align`] | `Widget.verticalAlignment/horizontalAlignment` |
//! | +0x134 | [`Widget::dirty`] | – |
//! | +0x138 | [`Widget::last_viewport`] | – |
//! | +0x140 / +0x144 | horizontal / vertical [`ScrollSlider`](WidgetKind::ScrollSlider) attached to this content widget | – |
//! | +0x148 | [`Widget::node`] | – |
//! | +0x14c / +0x14d | [`Widget::resize_edge`] | – |
//! | +0x150 | [`Widget::connections`] | – |
//! | +0x15c | [`Widget::enabled`] | – |

// Operand order and nesting follow the original's f32 evaluation and branch order.
#![allow(clippy::assign_op_pattern, clippy::collapsible_if, clippy::manual_clamp)]

use std::collections::{BTreeMap, BTreeSet};

use glam::{Affine2, IVec2, Mat2, Vec2};

/// Index of a [`Node`] in [`Gui::nodes`].
pub type NodeId = usize;
/// Index of a [`Widget`] in [`Gui::widgets`].
pub type WidgetId = usize;

// ---------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------

/// `Widget.flags` (`Widget+0x128`) bits.
pub mod flags {
    /// Movable along x (drag-move) and scrollable along x (ScrollSlider/ScrollButton
    /// target search, 0x00662e10 / 0x0067d8e0).
    pub const MOVABLE_X: u32 = 1 << 0;
    /// Movable along y; also enables the mouse wheel (0x0062a7b0).
    pub const MOVABLE_Y: u32 = 1 << 1;
    /// Stretch horizontally with the parent (0x0062af10); disables right anchoring.
    pub const STRETCH_X: u32 = 1 << 2;
    /// Stretch vertically with the parent (0x0062af10); disables bottom anchoring.
    pub const STRETCH_Y: u32 = 1 << 3;
    /// Pop-up list of a PopUpButton (0x0067dbf0).
    pub const POPUP_LIST: u32 = 1 << 4;
    /// Drag-and-drop source: becomes `Engine+0xfc` on left press (0x006527f0).
    pub const DRAGGABLE: u32 = 1 << 5;
    /// Resizable by its border (0x0062a9e0 / 0x0062ac20).
    pub const RESIZABLE: u32 = 1 << 7;
    /// The whole node counts as hit and is exempt from the widget clip (0x00636560).
    pub const HIT_ALL: u32 = 1 << 8;
}

/// `Node.flags` (`Node+0xc8`) bits used by the widget code.
pub mod node_flags {
    /// Ignored by event dispatch (0x00653620) and not returned by hit tests unless the
    /// caller asks (0x00636560 filter bit 2).
    pub const NO_EVENTS: u32 = 1 << 1;
    /// Excluded from hit testing, broadcasts, child-widget lists and resize propagation.
    pub const DISABLED: u32 = 1 << 2;
    /// Filtered by hit-test flag bit 4 (0x00636560).
    pub const HIT_FILTER_4: u32 = 1 << 3;
    /// Radio-group scope marker read by `Button::update` (0x006655d0); tentative.
    pub const RADIO_SCOPE: u32 = 1 << 4;
    /// Excluded from hit testing (0x00636560, bit 12).
    pub const NO_HIT: u32 = 1 << 12;
}

/// Event ids used as keys of the connection map (`Widget+0x150`).
pub mod event {
    /// Left button pressed on the widget (0x006527f0).
    pub const LEFT_PRESS: u32 = 2;
    /// Left button released (0x00652940).
    pub const LEFT_RELEASE: u32 = 3;
    /// Right button pressed (0x00652a70).
    pub const RIGHT_PRESS: u32 = 5;
    /// Right button released (0x00652b60).
    pub const RIGHT_RELEASE: u32 = 6;
    /// Mouse moved over the widget (0x00652c10).
    pub const MOUSE_MOVE: u32 = 0xc;
    /// Mouse entered (0x00652c10).
    pub const ENTER: u32 = 0xd;
    /// Mouse left (0x00652c10, 0x00652940).
    pub const LEAVE: u32 = 0xe;
    /// Content scrolled or dragged (0x0062ac20, 0x0062a7b0, sliders, scroll buttons).
    pub const SCROLLED: u32 = 0x11;
    /// Keyboard focus gained (0x00659df0).
    pub const FOCUS_GAINED: u32 = 0x12;
    /// Keyboard focus lost (0x00659df0, 0x006527f0).
    pub const FOCUS_LOST: u32 = 0x13;
    /// Edit text changed (0x00637f00, 0x006380c0, 0x00637850).
    pub const TEXT_CHANGED: u32 = 0x14;
    /// Button auto-repeat (0x00665b60).
    pub const REPEAT: u32 = 0x15;
}

/// Windows virtual-key codes handled by `Edit::onKeyDown` (0x006380c0).
pub mod vk {
    /// VK_BACK.
    pub const BACK: u16 = 0x08;
    /// VK_RETURN.
    pub const RETURN: u16 = 0x0d;
    /// VK_SHIFT (queried through `Engine::isKeyDown` 0x0043a3f0).
    pub const SHIFT: u16 = 0x10;
    /// VK_END.
    pub const END: u16 = 0x23;
    /// VK_HOME.
    pub const HOME: u16 = 0x24;
    /// VK_LEFT.
    pub const LEFT: u16 = 0x25;
    /// VK_RIGHT.
    pub const RIGHT: u16 = 0x27;
    /// VK_DELETE.
    pub const DELETE: u16 = 0x2e;
}

// ---------------------------------------------------------------------------------------
// Input types (.plx) — minimal; the integrator maps cw-formats' parser output to these.
// ---------------------------------------------------------------------------------------

/// What `PlxReader::readWidget` 0x00687440 (and the Button/Edit/List/Scroll/PopUp
/// readers, which create the subclass then read the same `Widget.*` fields through
/// 0x00687560) produce. One per `Node.widget` reference.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetSource {
    /// `Widget.name` (narrow) or `Widget.wname` (wide); the NamedObject name at +0xc.
    pub name: String,
    /// `Widget.caption` → +0x80 (0x0062ddc0, no refresh).
    pub caption: String,
    /// `Widget.innerBindPos` → +0x48.
    pub inner_bind_pos: Vec2,
    /// `Widget.innerBindSize` → +0x50.
    pub inner_bind_size: Vec2,
    /// `Widget.bindPos` → +0x58.
    pub bind_pos: Vec2,
    /// `Widget.bindSize` → +0x60.
    pub bind_size: Vec2,
    /// `Widget.framePos` → +0x68.
    pub frame_pos: Vec2,
    /// `Widget.frameSize` → +0x70.
    pub frame_size: Vec2,
    /// `Widget.bindMatrix`, 64 bytes, row-major D3D matrix (row-vector convention) →
    /// +0xa8, and its inverse at +0xe8 (0x00687ad0 copies it to both and inverts +0xe8
    /// with 0x0058c440). Only the 2D affine part is used.
    pub bind_matrix: [f32; 16],
    /// `Widget.horizontalAlignment` → +0x130 (0 left, 1 right, 2 centre).
    pub horizontal_alignment: i32,
    /// `Widget.verticalAlignment` → +0x12c (0 top, 1 bottom, 2 centre).
    pub vertical_alignment: i32,
    /// `Widget.flags` → +0x128.
    pub flags: u32,
    /// The chunk tag that created the widget, with its subclass fields.
    pub kind: WidgetSourceKind,
}

impl Default for WidgetSource {
    fn default() -> Self {
        // Defaults of the ctor 0x00627260.
        Self {
            name: String::new(),
            caption: String::new(),
            inner_bind_pos: Vec2::ZERO,
            inner_bind_size: Vec2::ONE,
            bind_pos: Vec2::ZERO,
            bind_size: Vec2::ONE,
            frame_pos: Vec2::ZERO,
            frame_size: Vec2::ONE,
            bind_matrix: IDENTITY16,
            horizontal_alignment: 0,
            vertical_alignment: 0,
            flags: 0,
            kind: WidgetSourceKind::Widget,
        }
    }
}

const IDENTITY16: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The top-level tag of the widget chunk (`PlxReader::read` 0x00681c70).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetSourceKind {
    /// `Widget` (0x00687440).
    Widget,
    /// `Button` (0x00683070): `Button.type` (0 push, 1 toggle, 2 radio).
    Button { button_type: i32 },
    /// `PopUpButton` (0x00684770): `Button.type`.
    PopUpButton { button_type: i32 },
    /// `ScrollButton` (0x00684970): `ScrollButton.direction` (0 +y, 1 −y, 2 +x, 3 −x),
    /// `Button.type`.
    ScrollButton { direction: i32, button_type: i32 },
    /// `ScrollSlider` (0x00684c30): `ScrollSlider.direction` (0 vertical, 1
    /// horizontal), `Button.type`.
    ScrollSlider { direction: i32, button_type: i32 },
    /// `Edit` (0x00683750).
    Edit,
    /// `ListWidget` (0x00683de0).
    ListWidget,
}

/// The node fields the widget code reads (`PlxReader::readNode` 0x00683f00 tags
/// `Node.name/wname/shape/transformation/display/widget/child/flags/variable`, plus the
/// Transformation and Display values it needs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeSource {
    /// `Node.name` / `Node.wname`.
    pub name: String,
    /// `Node.flags` → +0xc8 ([`node_flags`]).
    pub flags: u32,
    /// Current value of the Transformation's keyed translation (`+0x94[+0x68]`).
    pub translation: Vec2,
    /// Current value of `Transformation.pivot` (`+0x19c[+0x170]`): the point rotation and
    /// deformation act about (`Transformation::evaluate` 0x00678600);
    /// `GameController::onResize` also uses it as an origin offset.
    pub pivot: Vec2,
    /// Current value of `Transformation.rotation` (`+0x144[+0x118]`, degrees about x, y, z).
    pub rotation: [f32; 3],
    /// Current value of `Transformation.deformation` (`+0xec[+0xc0]`, row-major 4x4);
    /// `None` = identity.
    pub deformation: Option<[f32; 16]>,
    /// Display visibility attribute (`Display+0x94[+0x68]`); default true.
    pub visible: bool,
    /// Display clip attribute (`Display+0xec[+0xc0]`): children are hit only inside
    /// the node's own shape.
    pub clip: bool,
    /// Text of the node's TextShape (shape type 3), if its shape is one.
    pub text: Option<String>,
}

/// A node's shape as the hit test sees it (`Shape` vtable slots 4 and 5). The
/// integrator implements this for `SmoothMeshShape`/`TextShape`.
pub trait HitShape: std::fmt::Debug {
    /// Shape slot 4 `containsPoint` (point in the node's local, undeformed space).
    fn contains_point(&self, p: Vec2) -> bool;
    /// Local bounds (min, max) (Shape slot 8 `getBounds`).
    fn bounds(&self) -> (Vec2, Vec2);
    /// Shape slot 5: whether the owning widget's Deformer applies (the hit test then
    /// un-deforms the point first). Tentative meaning.
    fn is_deformed(&self) -> bool {
        false
    }
    /// Shape slot 13 `clone`, as `Node::clone` 0x006326d0 calls it (`[shape+0x34]`): a
    /// deep copy for the cloned node, or `None` when the shape cannot be copied.
    fn clone_shape(&self) -> Option<Box<dyn HitShape>> {
        None
    }
    /// The concrete shape, for a drawer that walks the node tree (the `.plx` loader's
    /// `SharedShape`); `None` for shapes that only hit-test.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// Glyph positions for the Edit caret (`0x0065e8d0`, the TextShape layout query used by
/// 0x00638b60, 0x00638610 and 0x006378e0). Implemented by the font module.
pub trait TextMetrics {
    /// Left edge x and advance width of character `index` of `text` in the edit node's
    /// local space; `index == len` gives the end position.
    fn glyph_box(&self, text: &[u16], index: usize) -> (f32, f32);
}

// ---------------------------------------------------------------------------------------
// Scene graph
// ---------------------------------------------------------------------------------------

/// `plasma::Node` (0xf0 bytes, ctor 0x00630470), reduced to what widgets use.
#[derive(Debug)]
pub struct Node {
    /// NamedObject name.
    pub name: String,
    /// +0x28.
    pub parent: Option<NodeId>,
    /// +0x2c child list, in draw order (last = topmost).
    pub children: Vec<NodeId>,
    /// Transformation translation key (`+0x38 → +0x94[+0x68]`).
    pub translation: Vec2,
    /// Transformation pivot (see [`NodeSource::pivot`]).
    pub pivot: Vec2,
    /// Transformation rotation, degrees about x, y, z.
    pub rotation: [f32; 3],
    /// Transformation deformation matrix (row-major 4x4).
    pub deformation: [f32; 16],
    /// `Node.flags` (+0xc8).
    pub flags: u32,
    /// Display visibility.
    pub visible: bool,
    /// Display clip.
    pub clip: bool,
    /// The node's own widget (+0x40).
    pub widget: Option<WidgetId>,
    /// Hit shape (+0x34).
    pub shape: Option<Box<dyn HitShape>>,
    /// TextShape text when the node's shape is a TextShape (type 3).
    pub text: Option<Vec<u16>>,
    /// Set by `FUN_006371b0` when a resize invalidates the node's deformed shapes.
    pub deform_dirty: bool,
}

/// A queued side effect for the integrator.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// A connected signal fired (`MemberFunctionConnection::invoke`).
    Signal { widget: WidgetId, event: u32 },
    /// `Node::setState(name)` (0x00636810); `snap` when the original also calls
    /// 0x00636f70/0x00636cb0/0x00636f10 to jump to the state's end.
    State { node: NodeId, name: &'static str, snap: bool },
    /// Edit text replaced; the TextShape must rebuild (Shape slot 1).
    TextRebuild { node: NodeId },
    /// A non-silent `setPosition` (0x006295a0 with `silent == 0`): the original also
    /// calls 0x00637260 (unresolved, likely "transformation changed" notification).
    TransformNotify { node: NodeId },
    /// The caret/selection of an Edit changed (0x00638610 `updateCaret`).
    CaretChanged { widget: WidgetId },
}

/// A connection in the event map (`Widget+0x150`, `MemberFunctionConnection<T>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Connection {
    /// Byte at connection+4: when set, the engine-side dispatch (0x00653620 and the
    /// mouse injectors) skips the call while the widget is disabled.
    pub only_when_enabled: bool,
}

// ---------------------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------------------

/// `plasma::Widget` (vtable 0x0071e724, ctor 0x00627260).
#[derive(Clone, Debug)]
pub struct Widget {
    /// NamedObject name (+0xc).
    pub name: String,
    /// +0x148.
    pub node: NodeId,
    /// +0x38 class id written by the subclass ctors.
    pub class_id: i32,
    /// +0x3c; hit test clips to the widget rect when set (and the engine flag is on).
    pub clip_hit: bool,
    /// +0x48.
    pub inner_bind_pos: Vec2,
    /// +0x50.
    pub inner_bind_size: Vec2,
    /// +0x58.
    pub bind_pos: Vec2,
    /// +0x60.
    pub bind_size: Vec2,
    /// +0x68.
    pub frame_pos: Vec2,
    /// +0x70.
    pub frame_size: Vec2,
    /// +0x78.
    pub min_frame_size: Vec2,
    /// +0x80.
    pub caption: String,
    /// +0x98.
    pub drag_start_cursor: Vec2,
    /// +0xa0.
    pub drag_start_translation: Vec2,
    /// +0xe8: the inverse of `Widget.bindMatrix` (2D affine part); maps the widget's
    /// frame space into its node's space (`0x0062cfd0`: bind · node · ancestors).
    pub bind_matrix: Affine2,
    /// +0x128.
    pub flags: u32,
    /// +0x12c.
    pub v_align: i32,
    /// +0x130.
    pub h_align: i32,
    /// +0x134.
    pub dirty: bool,
    /// +0x138.
    pub last_viewport: IVec2,
    /// +0x140.
    pub h_slider: Option<WidgetId>,
    /// +0x144.
    pub v_slider: Option<WidgetId>,
    /// +0x14c / +0x14d: −1 left/top edge, 1 right/bottom edge, 0 none.
    pub resize_edge: [i8; 2],
    /// +0x150.
    pub connections: BTreeMap<u32, Connection>,
    /// +0x15c.
    pub enabled: bool,
    /// Subclass data.
    pub kind: WidgetKind,
}

/// The concrete class of a widget.
#[derive(Clone, Debug)]
pub enum WidgetKind {
    /// `plasma::Widget`.
    Base,
    /// `plasma::Button` (vtable 0x0071ed84, 0x238 bytes).
    Button(ButtonData),
    /// `plasma::PopUpButton : Button` (vtable 0x0071f64c, 0x240 bytes).
    PopUpButton(ButtonData, PopUpData),
    /// `plasma::ScrollButton : Button` (vtable 0x0071f4cc, 0x24c bytes).
    ScrollButton(ButtonData, ScrollButtonData),
    /// `plasma::ScrollSlider : Button` (vtable 0x0071ebfc, 0x254 bytes).
    ScrollSlider(ButtonData, ScrollSliderData),
    /// `plasma::Edit` (vtable 0x0071e83c, 0x17c bytes).
    Edit(EditData),
    /// `plasma::ListWidget` (vtable 0x0071f58c, 0x160 bytes; only clone/dtor differ).
    ListWidget,
    /// A `cube::*Widget` built by the GameController.
    Game(GameWidget),
}

/// `plasma::Button` fields (ctor 0x00664d60).
#[derive(Clone, Debug, Default)]
pub struct ButtonData {
    /// +0x220: the other radio buttons of the group (filled by `update`).
    pub radio_group: Vec<WidgetId>,
    /// +0x228.
    pub pressed: bool,
    /// +0x229.
    pub checked: bool,
    /// +0x22c `Button.type`: 0 push, 1 toggle, 2 radio.
    pub button_type: i32,
    /// +0x230 auto-repeat accumulator (ms).
    pub repeat_acc: i32,
    /// +0x234 auto-repeat interval (ms).
    pub repeat_interval: i32,
}

/// The eight node state names of a Button (+0x160..+0x208).
pub mod button_state {
    /// +0x160.
    pub const PRESS: &str = "button:press";
    /// +0x178.
    pub const RELEASE: &str = "button:release";
    /// +0x190.
    pub const ENTER: &str = "button:enter";
    /// +0x1a8.
    pub const LEAVE: &str = "button:leave";
    /// +0x1c0.
    pub const PRESS_CHECKED: &str = "button:press:checked";
    /// +0x1d8.
    pub const RELEASE_CHECKED: &str = "button:release:checked";
    /// +0x1f0.
    pub const ENTER_CHECKED: &str = "button:enter:checked";
    /// +0x208.
    pub const LEAVE_CHECKED: &str = "button:leave:checked";
}

/// `PopUpButton` fields (ctor 0x0067dae0).
#[derive(Clone, Debug)]
pub struct PopUpData {
    /// +0x238: the ListWidget shown as the pop-up.
    pub list: Option<WidgetId>,
    /// +0x23c: copy the chosen item's caption into this button (ctor sets true).
    pub adopt_caption: bool,
}

/// `ScrollButton` fields (ctor 0x0067d450).
#[derive(Clone, Debug)]
pub struct ScrollButtonData {
    /// +0x238: 0 +y, 1 −y, 2 +x, 3 −x.
    pub direction: i32,
    /// +0x23c, 10.0 in the ctor; not read by the ported code.
    pub unused_step: f32,
    /// +0x240: scroll target.
    pub target: Option<WidgetId>,
    /// +0x244, zeroed on press.
    pub counter: i32,
}

/// `ScrollSlider` fields (ctor 0x006627f0).
#[derive(Clone, Debug)]
pub struct ScrollSliderData {
    /// +0x238: 1 horizontal, anything else vertical.
    pub direction: i32,
    /// +0x23c / +0x244: value of `FUN_00631db0(1)` at press (unresolved, not read back).
    pub press_origin: Vec2,
    /// +0x24c: the scrolled content widget.
    pub target: Option<WidgetId>,
}

/// `Edit` fields (ctor 0x00637700).
#[derive(Clone, Debug, Default)]
pub struct EditData {
    /// +0x160: the node holding the TextShape (the edit's own node when its shape is a
    /// TextShape).
    pub text_node: Option<NodeId>,
    /// +0x164.
    pub filter: Option<EditFilter>,
    /// +0x168: a press started a selection drag.
    pub selecting: bool,
    /// +0x16c: node translation at `update` (horizontal scroll origin).
    pub origin_translation: Vec2,
    /// +0x174 caret index.
    pub caret: i32,
    /// +0x178 signed selection length (from the caret).
    pub selection: i32,
}

/// `plasma::EditFilter` subclasses (one slot, `accept`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFilter {
    /// vtable 0x006fcc90, accept 0x00637d10.
    Integer,
    /// vtable 0x006fcc98, accept 0x00637e20.
    UnsignedInteger,
    /// vtable 0x006fcca0, accept 0x00637c60.
    Float,
    /// vtable 0x006fcca8, accept 0x00637d90.
    UnsignedFloat,
}

impl EditFilter {
    /// Slot 0 `accept(wstring)`.
    pub fn accept(self, s: &[u16]) -> bool {
        match self {
            // 0x00637d10: empty ok; optional '-' then digits only.
            EditFilter::Integer => {
                if s.is_empty() {
                    return true;
                }
                let start = usize::from(s[0] == 0x2d);
                s[start..].iter().all(|&c| (0x30..=0x39).contains(&c))
            }
            // 0x00637e20.
            EditFilter::UnsignedInteger => s.iter().all(|&c| (0x30..=0x39).contains(&c)),
            // 0x00637c60: optional '-', digits, at most one '.' or ',' in total.
            EditFilter::Float => {
                if s.is_empty() {
                    return true;
                }
                let start = usize::from(s[0] == 0x2d);
                float_body(&s[start..])
            }
            // 0x00637d90.
            EditFilter::UnsignedFloat => float_body(s),
        }
    }
}

fn float_body(s: &[u16]) -> bool {
    let mut seen_sep = false;
    for &c in s {
        if c == 0x2e || c == 0x2c {
            if seen_sep {
                return false;
            }
            seen_sep = true;
        } else if !(0x30..=0x39).contains(&c) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Game widgets
// ---------------------------------------------------------------------------------------

/// The `cube::*Widget` classes (client-classes.md "Game widgets").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameWidgetClass {
    /// vtable 0x006fcbd4, ctor 0x0040ecd0, 0x178 bytes.
    Adaption,
    /// vtable 0x006fd364, ctor 0x0042f190, 0x3c8 bytes.
    BlueprintPreview,
    /// vtable 0x006fcedc, ctor 0x00424e80, 0x168 bytes.
    CharacterPreview,
    /// vtable 0x006fd0e4, ctor 0x00427ce0, 0x1a8 bytes.
    CharacterStyle,
    /// vtable 0x006fd5b4, ctor 0x00434d90, 0x164 bytes.
    Character,
    /// vtable 0x006fd78c, ctor 0x004393b0, 0x190 bytes.
    Chat,
    /// vtable 0x006ffc9c, ctor 0x0044e910, 0x174 bytes.
    Enchant,
    /// vtable 0x00702bb4, ctor 0x004c1bb0, 0x1b8 bytes (3 instances).
    Inventory,
    /// vtable 0x00702fe4, ctor 0x004c95a0, 0x164 bytes.
    MapOverlay,
    /// vtable 0x007030dc, ctor 0x004ce180, 0x170 bytes.
    Objective,
    /// vtable 0x0070330c, ctor 0x004cf3c0, 0x208 bytes.
    Options,
    /// vtable 0x00703594, ctor 0x004d4f10, 0x180 bytes.
    Preview,
    /// vtable 0x00703a34, ctor 0x004dd750, 0x194 bytes.
    Skill,
    /// vtable 0x00703e34, ctor 0x004e5c90, 0x1e0 bytes (2 instances).
    Speech,
    /// vtable 0x00711f24, ctor 0x0051c310, 0x16c bytes (2 instances).
    Sprite,
    /// vtable 0x0071a124, ctor 0x00583270, 0x168 bytes.
    StartMenu,
    /// vtable 0x0071a22c, ctor 0x00583b40, 0x164 bytes.
    Statistics,
    /// vtable 0x0071a564, ctor 0x00587660, 0x168 bytes.
    System,
    /// vtable 0x0071a664, ctor 0x00587f70, 0x440 bytes.
    Voxel,
    /// vtable 0x0071e09c, ctor 0x00605a20, 0x16c bytes.
    WorldPreview,
}

impl GameWidgetClass {
    /// Whether the class overrides slot 5 with the shared `containsPoint` 0x004c59a0.
    pub fn uses_rect_contains(self) -> bool {
        matches!(
            self,
            Self::CharacterPreview | Self::Inventory | Self::Sprite | Self::WorldPreview
        )
    }

    /// Summary of the class's slot 1 per-frame body (game logic; belongs to cw-client).
    pub fn frame_update_summary(self) -> &'static str {
        match self {
            Self::Adaption => "0x0040f8f0 (6.4 KB): adaption (item re-roll) window: title \"Adaption\", \"Adapt\"/\"Goodbye!\" buttons, cost in platinum coins, draws the item model with CubeShader",
            Self::BlueprintPreview => "0x0042f910 (9.2 KB): draws the blueprint's item model (Model::draw) in a frame, \"Unknown\" label",
            Self::CharacterPreview => "0x00425450 (8.8 KB): character-select preview: \"New character\" or level/specialization text, draws the creature model with lighting/fog uniforms",
            Self::CharacterStyle => "0x00428e40 (10 KB): character creation rows Race/Gender/Class/Face/Haircut/Hair color and their value labels",
            Self::Character => "0x00434e30 (16.7 KB): character sheet: level, power, armor, resistance, crit, tempo, weapon/armor rating from Creature stat helpers",
            Self::Chat => "0x00439730 (2.2 KB): chat log layout/scroll (no strings)",
            Self::Enchant => "0x0044ea30 (7.8 KB): identification window: \"Identification\", \"Identify\"/\"Goodbye!\", cost",
            Self::Inventory => "0x004c2050 (14.3 KB): draws item icons/models per grid cell, coin count, frame",
            Self::MapOverlay => "0x004c9680 (6.6 KB): map overlay drawn from World::baseHeight / Cell::falloff",
            Self::Objective => "slot 1 not overridden",
            Self::Options => "0x004d0230 (16.7 KB): options rows (Mode, Resolution, Anti-aliasing, Render Distance, volumes, camera, Invert Y, FPS Limit, Language) and their values",
            Self::Preview => "0x004d50a0 (5.7 KB): item tooltip preview with rarity stars and \"Currently equipped\"",
            Self::Skill => "0x004dd810 (7.9 KB): skill tree: points, \"Learn\"/\"Cancel\", cost, \"Not enough money.\", \"Requires class trainer.\"",
            Self::Speech => "0x004e5f90 (1.4 KB): speech bubble text; plays sound 0x32 with random pitch",
            Self::Sprite => "0x0051c3d0 (0.9 KB): draws a model sprite (alpha, shininess)",
            Self::StartMenu => "0x00583320 (2 KB): captions \"Start Game\", \"Options\", \"Exit\"",
            Self::Statistics => "0x00583be0 (15 bytes): trivial",
            Self::System => "0x00587710 (2 KB): captions \"Options\", \"Start Menu\", \"Exit Game\"",
            Self::Voxel => "0x00588500 (16 KB): weapon customization (\"Upgrades\"), voxel model editing and drawing",
            Self::WorldPreview => "0x00605ae0 (11.9 KB): world-select preview: \"New world\", explored %, seed; draws the world model",
        }
    }
}

/// A `cube::*Widget` instance. `children` maps the original member offset (for
/// example `0x170`) to the child node the class stores there; the layout bodies read
/// them by offset.
#[derive(Clone, Debug)]
pub struct GameWidget {
    /// Class.
    pub class: GameWidgetClass,
    /// Child nodes by member offset.
    pub children: BTreeMap<u32, NodeId>,
    /// InventoryWidget only: +0x1a8 / +0x1ac cell size (ints).
    pub cell_size: IVec2,
    /// InventoryWidget only: +0x198 vector (per-tab first row), +0x1b4 tab, +0x190
    /// selected index.
    pub tab_rows: Vec<i32>,
    /// See [`GameWidget::tab_rows`].
    pub tab: i32,
    /// See [`GameWidget::tab_rows`].
    pub selected: i32,
}

impl GameWidget {
    /// An instance with no children.
    pub fn new(class: GameWidgetClass) -> Self {
        Self {
            class,
            children: BTreeMap::new(),
            cell_size: IVec2::ZERO,
            tab_rows: Vec::new(),
            tab: 0,
            selected: 0,
        }
    }
}

// ---------------------------------------------------------------------------------------
// The engine side (`plasma::Engine` input state)
// ---------------------------------------------------------------------------------------

/// The widget tree plus the `plasma::Engine` input state that routes events to it.
#[derive(Debug, Default)]
pub struct Gui {
    /// Node arena.
    pub nodes: Vec<Node>,
    /// Widget arena.
    pub widgets: Vec<Widget>,
    /// Engine+0xb4 root node.
    pub root: Option<NodeId>,
    /// Engine+0xc0 overlay layer that pop-ups are moved into (root when absent).
    pub overlay: Option<NodeId>,
    /// Engine root Transformation matrix `+0x1f0` (identity unless the page scales).
    pub root_matrix: Affine2,
    /// Engine+0x10c / +0x110.
    pub viewport: IVec2,
    /// Engine+0xd4 / +0xd8.
    pub cursor: Vec2,
    /// Engine+0xdc / +0xe0.
    pub prev_cursor: Vec2,
    /// Engine+0xc4.
    pub hovered: Option<NodeId>,
    /// Engine+0xc8.
    pub captured: Option<NodeId>,
    /// Engine+0xcc.
    pub focused: Option<WidgetId>,
    /// Engine+0xd0.
    pub popup: Option<WidgetId>,
    /// Engine+0xf4 bit 0: left button down (set by the injectors).
    pub left_down: bool,
    /// Engine+0xf4 bit 1: right button down.
    pub right_down: bool,
    /// Engine+0xf4 bit 2: drag-move active, read by `onMouseDrag` 0x0062ac20. Nothing in
    /// Cube.exe sets it: the only writers of Engine+0xf4 are the left/right button
    /// injectors (bits 0 and 1: 0x006527f0 / 0x00652940 / 0x00652a70 / 0x00652b60), so
    /// drag-move of movable widgets is dead code in the shipped client and this stays
    /// false (kept so the ported `onMouseDrag` branch can be exercised).
    pub drag_move_active: bool,
    /// Engine+0xf8.
    pub last_key: u16,
    /// Engine+0xfc: the draggable widget under the last left press.
    pub drag_widget: Option<WidgetId>,
    /// Engine+0x100: wheel steps of this frame.
    pub wheel: i32,
    /// Engine+0xe4: frame time in ms.
    pub frame_ms: i32,
    /// Engine+0xec: keys currently down (queried by `isKeyDown` 0x0043a3f0).
    pub keys_down: BTreeSet<u16>,
    /// Engine+0x18c bit 1: clip hit tests to widget rects.
    pub clip_hits_to_widgets: bool,
    /// Queued side effects.
    pub events: Vec<UiEvent>,
}

/// Effective extent on one axis (`0x00627d50`, the scalar part of 0x0062de60):
/// `(bind + max(inner + frame − bind, max(inner + min − bind, 0))) − inner`.
pub fn effective_axis(inner: f32, frame: f32, min: f32, bind: f32) -> f32 {
    (bind + stretch_axis(inner, frame, min, bind)) - inner
}

/// Deformed extent of the inner (stretchable) rect on one axis
/// (`0x0062bb90`/`0x00627e40`): `max(inner + frame − bind, max(inner + min − bind, 0))`.
pub fn stretch_axis(inner: f32, frame: f32, min: f32, bind: f32) -> f32 {
    let mut lo = (min + inner) - bind;
    let mut s = (frame + inner) - bind;
    // `if (lo < 0.0) lo = 0.0;` — false on NaN, like the original's comiss/jbe.
    if lo < 0.0 {
        lo = 0.0;
    }
    if s < lo {
        s = lo;
    }
    s
}

/// The GUI vertex shader's deformation constants for one widget
/// (`widgetBindPos/Size`, `deformedWidgetPos/Size` of shader 06; the same values
/// `Widget::deformPoint` 0x00627e40 computes on the CPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeformUniforms {
    /// `innerBindPos`.
    pub bind_pos: Vec2,
    /// `innerBindSize`.
    pub bind_size: Vec2,
    /// `innerBindPos + framePos − bindPos`.
    pub deformed_pos: Vec2,
    /// [`stretch_axis`] on both axes.
    pub deformed_size: Vec2,
}

impl DeformUniforms {
    /// True when deformation is the identity (0x00627e40's early-out: deformed size
    /// equals the bind size and deformed position equals the bind position).
    pub fn is_identity(&self) -> bool {
        self.deformed_size.x == self.bind_size.x
            && self.deformed_size.y == self.bind_size.y
            && self.deformed_pos.x == self.bind_pos.x
            && self.deformed_pos.y == self.bind_pos.y
    }

    /// `Widget::deformPoint` 0x00627e40 on a point already in widget bind space: a
    /// 9-slice stretch. Points left of the inner rect move with the frame offset, points
    /// inside are stretched, points right of it move by the full stretch.
    pub fn deform(&self, p: Vec2) -> Vec2 {
        if self.is_identity() {
            return p;
        }
        let axis = |p: f32, bpos: f32, bsize: f32, dpos: f32, dsize: f32| -> f32 {
            let t = if bpos <= p {
                if bpos + bsize <= p { 1.0 } else { (p - bpos) / bsize }
            } else {
                0.0
            };
            ((p + dpos) - bpos) + t * (dsize - bsize)
        };
        Vec2::new(
            axis(p.x, self.bind_pos.x, self.bind_size.x, self.deformed_pos.x, self.deformed_size.x),
            axis(p.y, self.bind_pos.y, self.bind_size.y, self.deformed_pos.y, self.deformed_size.y),
        )
    }

    /// `Widget::undeformPoint`, the Deformer's slot 1 0x0062e180 (the hit test uses it to
    /// un-deform the cursor): the same 9-slice map run backwards, with the parameter taken
    /// in deformed space:
    /// `t = 0` left of `dpos`, `1` right of `dpos + dsize`, else `(q - dpos) / dsize`;
    /// `p = ((bpos + q) - dpos) + t·(bsize - dsize)`.
    pub fn undeform(&self, q: Vec2) -> Vec2 {
        if self.is_identity() {
            return q;
        }
        let axis = |q: f32, bpos: f32, bsize: f32, dpos: f32, dsize: f32| -> f32 {
            let t = if dpos <= q {
                if dsize + dpos <= q { 1.0 } else { (q - dpos) / dsize }
            } else {
                0.0
            };
            ((bpos + q) - dpos) + t * (bsize - dsize)
        };
        Vec2::new(
            axis(q.x, self.bind_pos.x, self.bind_size.x, self.deformed_pos.x, self.deformed_size.x),
            axis(q.y, self.bind_pos.y, self.bind_size.y, self.deformed_pos.y, self.deformed_size.y),
        )
    }
}

/// `Transformation::evaluate` 0x00678600 (vtable 0x0071f344 slot 1): the matrix of
/// [`crate::shape::plasma_transform_matrix`] with its third column then forced to
/// `(0, 0, 1, 0)` (so z passes through unchanged). The original also stores a copy at
/// `+0x1f0` and inverts it (0x0058c440).
pub fn evaluate_transformation(t: Vec2, r: [f32; 3], p: Vec2, d: &[f32; 16]) -> [f32; 16] {
    let mut m = crate::shape::plasma_transform_matrix(t.to_array(), p.to_array(), r, d);
    m[2] = 0.0;
    m[6] = 0.0;
    m[10] = 1.0;
    m[14] = 0.0;
    m
}

/// Converts a D3D row-major (row-vector) 4x4 matrix to its 2D affine part.
pub fn affine_from_d3d(m: &[f32; 16]) -> Affine2 {
    // Row-vector convention: p' = p * M, so x' = x*m00 + y*m10 + m30.
    Affine2::from_mat2_translation(
        Mat2::from_cols(Vec2::new(m[0], m[1]), Vec2::new(m[4], m[5])),
        Vec2::new(m[12], m[13]),
    )
}

impl Gui {
    /// An empty GUI.
    pub fn new() -> Self {
        Self::default()
    }

    // --- construction ------------------------------------------------------------------

    /// `Engine::createNode` 0x0064f4e0 reduced: appends a node under `parent` (last =
    /// topmost). The first node without a parent becomes the root.
    pub fn add_node(&mut self, parent: Option<NodeId>, src: NodeSource) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: src.name,
            parent,
            children: Vec::new(),
            translation: src.translation,
            pivot: src.pivot,
            rotation: src.rotation,
            deformation: src.deformation.unwrap_or(IDENTITY16),
            flags: src.flags,
            visible: src.visible,
            clip: src.clip,
            widget: None,
            shape: None,
            text: src.text.map(|t| t.encode_utf16().collect()),
            deform_dirty: false,
        });
        match parent {
            Some(p) => self.nodes[p].children.push(id),
            None => {
                if self.root.is_none() {
                    self.root = Some(id);
                }
            }
        }
        id
    }

    /// A visible node named `name` with no other data.
    pub fn add_plain_node(&mut self, parent: Option<NodeId>, name: &str) -> NodeId {
        self.add_node(parent, NodeSource { name: name.into(), visible: true, ..Default::default() })
    }

    /// `Widget::Widget` 0x00627260 plus the subclass ctors (Button 0x00664d60,
    /// PopUpButton 0x0067dae0, ScrollButton 0x0067d450, ScrollSlider 0x006627f0,
    /// Edit 0x00637700, ListWidget 0x0067d9f0), with the `.plx` fields applied.
    /// Sets `node->+0x40 = widget`.
    pub fn add_widget(&mut self, node: NodeId, src: &WidgetSource) -> WidgetId {
        let button = |t: i32| ButtonData { button_type: t, ..Default::default() };
        let (class_id, kind) = match src.kind {
            WidgetSourceKind::Widget => (0, WidgetKind::Base),
            WidgetSourceKind::Button { button_type } => (1, WidgetKind::Button(button(button_type))),
            WidgetSourceKind::PopUpButton { button_type } => (
                1,
                WidgetKind::PopUpButton(button(button_type), PopUpData { list: None, adopt_caption: true }),
            ),
            WidgetSourceKind::ScrollButton { direction, button_type } => (
                1,
                WidgetKind::ScrollButton(
                    button(button_type),
                    ScrollButtonData { direction, unused_step: 10.0, target: None, counter: 0 },
                ),
            ),
            WidgetSourceKind::ScrollSlider { direction, button_type } => (
                1,
                WidgetKind::ScrollSlider(
                    button(button_type),
                    ScrollSliderData { direction, press_origin: Vec2::ZERO, target: None },
                ),
            ),
            WidgetSourceKind::Edit => (2, WidgetKind::Edit(EditData::default())),
            WidgetSourceKind::ListWidget => (4, WidgetKind::ListWidget),
        };
        self.push_widget(node, src, class_id, kind)
    }

    /// Creates a `cube::*Widget` on `node` (the game ctors call `Widget::Widget` and
    /// then set their own vtable; class id stays 0).
    pub fn add_game_widget(&mut self, node: NodeId, src: &WidgetSource, game: GameWidget) -> WidgetId {
        self.push_widget(node, src, 0, WidgetKind::Game(game))
    }

    fn push_widget(&mut self, node: NodeId, src: &WidgetSource, class_id: i32, kind: WidgetKind) -> WidgetId {
        let id = self.widgets.len();
        self.widgets.push(Widget {
            name: src.name.clone(),
            node,
            class_id,
            clip_hit: false,
            inner_bind_pos: src.inner_bind_pos,
            inner_bind_size: src.inner_bind_size,
            bind_pos: src.bind_pos,
            bind_size: src.bind_size,
            frame_pos: src.frame_pos,
            frame_size: src.frame_size,
            min_frame_size: Vec2::ZERO,
            caption: src.caption.clone(),
            drag_start_cursor: Vec2::ZERO,
            drag_start_translation: Vec2::ZERO,
            // +0xe8 holds the inverse of the file's matrix (0x00687ad0).
            bind_matrix: affine_from_d3d(&src.bind_matrix).inverse(),
            flags: src.flags,
            v_align: src.vertical_alignment,
            h_align: src.horizontal_alignment,
            dirty: false,
            last_viewport: IVec2::ZERO,
            h_slider: None,
            v_slider: None,
            resize_edge: [0, 0],
            connections: BTreeMap::new(),
            enabled: true,
            kind,
        });
        self.nodes[node].widget = Some(id);
        id
    }

    /// `Widget::connect<T>` (0x00427bc0 / 0x004c1a90 / 0x004cf2a0 / 0x004576f0).
    pub fn connect(&mut self, w: WidgetId, event: u32, conn: Connection) {
        self.widgets[w].connections.insert(event, conn);
    }

    // --- tree helpers ------------------------------------------------------------------

    /// `Node+0x44`: the widget that handles a node's events — its own, else the nearest
    /// ancestor's (tentative: the field is filled at load time, not read).
    pub fn owner(&self, n: NodeId) -> Option<WidgetId> {
        let mut cur = Some(n);
        while let Some(c) = cur {
            if let Some(w) = self.nodes[c].widget {
                return Some(w);
            }
            cur = self.nodes[c].parent;
        }
        None
    }

    /// `0x0062b400`: the nearest ancestor node (excluding the widget's own) that has a
    /// widget.
    pub fn parent_widget(&self, w: WidgetId) -> Option<WidgetId> {
        let mut cur = self.nodes[self.widgets[w].node].parent;
        while let Some(c) = cur {
            if let Some(pw) = self.nodes[c].widget {
                return Some(pw);
            }
            cur = self.nodes[c].parent;
        }
        None
    }

    /// `0x00629140` / `0x006290d0`: the widgets directly below `w`: for each child node,
    /// its widget if it has one, else recurse; nodes with [`node_flags::DISABLED`] are
    /// skipped with their subtree.
    pub fn child_widgets(&self, w: WidgetId) -> Vec<WidgetId> {
        let mut out = Vec::new();
        let own = self.widgets[w].node;
        self.collect_child_widgets(own, own, &mut out);
        out
    }

    fn collect_child_widgets(&self, n: NodeId, own: NodeId, out: &mut Vec<WidgetId>) {
        let node = &self.nodes[n];
        if node.flags & node_flags::DISABLED != 0 {
            return;
        }
        if n != own {
            if let Some(w) = node.widget {
                out.push(w);
                return;
            }
        }
        for &c in &node.children {
            self.collect_child_widgets(c, own, out);
        }
    }

    /// `0x006326a0`: whether `ancestor` is `n` or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, n: Option<NodeId>) -> bool {
        let mut cur = n;
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes[c].parent;
        }
        false
    }

    /// The node's Transformation matrix (`Transformation::evaluate` 0x00678600, the value
    /// at `+0x1b0`) as a 2D affine map.
    ///
    /// Performance note for a future optimiser: the original caches the matrix and
    /// re-evaluates only when an attribute changes (slot 1 after each write); this
    /// re-evaluates on every query.
    fn node_local(&self, n: NodeId) -> Affine2 {
        let node = &self.nodes[n];
        affine_from_d3d(&evaluate_transformation(node.translation, node.rotation, node.pivot, &node.deformation))
    }

    /// World transform of a node (the product of the Transformation matrices up to the
    /// root, `0x0062cfd0` without the widget terms), prefixed by the root matrix.
    pub fn node_world(&self, n: NodeId) -> Affine2 {
        let mut m = self.node_local(n);
        let mut cur = self.nodes[n].parent;
        while let Some(p) = cur {
            m = self.node_local(p) * m;
            cur = self.nodes[p].parent;
        }
        self.root_matrix * m
    }

    /// `0x0062c5b0`: world transform of the widget frame: node world · bind matrix ·
    /// translate(framePos).
    pub fn widget_world(&self, w: WidgetId) -> Affine2 {
        let wd = &self.widgets[w];
        self.node_world(wd.node) * wd.bind_matrix * Affine2::from_translation(wd.frame_pos)
    }

    // --- geometry (Widget non-virtual helpers) -------------------------------------------

    /// `0x0062b510 getPosition`: the widget frame origin relative to the parent widget's
    /// frame origin, in world units.
    pub fn get_position(&self, w: WidgetId) -> Vec2 {
        let own = self.widget_world(w).translation;
        match self.parent_widget(w) {
            Some(p) => own - self.widget_world(p).translation,
            None => own,
        }
    }

    /// `0x00627d50`: effective local size, `(bindSize + stretch) − innerBindSize`.
    pub fn local_size(&self, w: WidgetId) -> Vec2 {
        let wd = &self.widgets[w];
        Vec2::new(
            effective_axis(wd.inner_bind_size.x, wd.frame_size.x, wd.min_frame_size.x, wd.bind_size.x),
            effective_axis(wd.inner_bind_size.y, wd.frame_size.y, wd.min_frame_size.y, wd.bind_size.y),
        )
    }

    /// `0x0062de60 getSize`: [`Gui::local_size`] through the widget's world linear part.
    pub fn get_size(&self, w: WidgetId) -> Vec2 {
        self.widget_world(w).matrix2 * self.local_size(w)
    }

    /// `0x0062f600`.
    pub fn width(&self, w: WidgetId) -> f32 {
        self.get_size(w).x
    }

    /// `0x006291d0`.
    pub fn height(&self, w: WidgetId) -> f32 {
        self.get_size(w).y
    }

    /// `0x006295a0 setPosition(pos, silent)`: moves the node's translation key so the
    /// frame origin lands at `pos` (relative to the parent widget). Exact equality with
    /// the current position returns early.
    pub fn set_position(&mut self, w: WidgetId, pos: Vec2, silent: bool) {
        let old = self.get_position(w);
        if pos.x == old.x && pos.y == old.y {
            return;
        }
        let node = self.widgets[w].node;
        let parent_linear = match self.nodes[node].parent {
            Some(p) => self.node_world(p).matrix2,
            None => self.root_matrix.matrix2,
        };
        let delta = parent_linear.inverse() * (pos - old);
        self.nodes[node].translation += delta;
        if !silent {
            self.events.push(UiEvent::TransformNotify { node });
        }
    }

    /// `0x0062a650`: `setPosition((x, y), silent)`.
    pub fn set_position_xy(&mut self, w: WidgetId, x: f32, y: f32, silent: bool) {
        self.set_position(w, Vec2::new(x, y), silent);
    }

    /// `0x0062bb90 setSize(size, propagate)`: stores the frame size (in local units),
    /// propagates the change of the stretch extent to child nodes/widgets and calls
    /// slot 10 `layout`.
    pub fn set_size(&mut self, w: WidgetId, size: Vec2, propagate: bool) {
        let (old, world) = {
            let wd = &self.widgets[w];
            let old = Vec2::new(
                stretch_axis(wd.inner_bind_size.x, wd.frame_size.x, wd.min_frame_size.x, wd.bind_size.x),
                stretch_axis(wd.inner_bind_size.y, wd.frame_size.y, wd.min_frame_size.y, wd.bind_size.y),
            );
            (old, self.widget_world(w).matrix2)
        };
        let local = world.inverse() * size;
        self.widgets[w].frame_size = local;
        let new = {
            let wd = &self.widgets[w];
            Vec2::new(
                stretch_axis(wd.inner_bind_size.x, wd.frame_size.x, wd.min_frame_size.x, wd.bind_size.x),
                stretch_axis(wd.inner_bind_size.y, wd.frame_size.y, wd.min_frame_size.y, wd.bind_size.y),
            )
        };
        let delta = world * Vec2::new(new.x - old.x, new.y - old.y);
        let node = self.widgets[w].node;
        self.propagate_resize(w, node, Vec2::ZERO, delta, propagate);
        self.layout(w);
    }

    /// `0x0062bb20 setRect(x, y, w, h, propagate)`: `setPosition` (not silent) then
    /// `setSize`.
    pub fn set_rect(&mut self, w: WidgetId, x: f32, y: f32, sw: f32, sh: f32, propagate: bool) {
        self.set_position(w, Vec2::new(x, y), false);
        self.set_size(w, Vec2::new(sw, sh), propagate);
    }

    /// `0x0062ba50`: walks the nodes under `w`'s node; nodes of `w` itself (or with no
    /// widget) get their deformed shapes invalidated and, when `delta` is non-zero,
    /// recurse; nodes of another widget get that widget's slot 9 `onParentResized`
    /// when `propagate` is set.
    fn propagate_resize(&mut self, w: WidgetId, n: NodeId, offset: Vec2, delta: Vec2, propagate: bool) {
        if self.nodes[n].flags & node_flags::DISABLED != 0 {
            return;
        }
        match self.nodes[n].widget {
            Some(other) if other != w => {
                if propagate {
                    self.on_parent_resized(other, offset, delta);
                }
            }
            _ => {
                // FUN_006371b0: invalidate the node's deformed geometry.
                self.nodes[n].deform_dirty = true;
                if delta.x != 0.0 || delta.y != 0.0 {
                    self.widgets[w].dirty = true;
                    let children = self.nodes[n].children.clone();
                    for c in children {
                        self.propagate_resize(w, c, offset, delta, propagate);
                    }
                }
            }
        }
    }

    /// Deformation constants of a widget (see [`DeformUniforms`]).
    pub fn deform_uniforms(&self, w: WidgetId) -> DeformUniforms {
        let wd = &self.widgets[w];
        DeformUniforms {
            bind_pos: wd.inner_bind_pos,
            bind_size: wd.inner_bind_size,
            deformed_pos: Vec2::new(
                (wd.inner_bind_pos.x + wd.frame_pos.x) - wd.bind_pos.x,
                (wd.inner_bind_pos.y + wd.frame_pos.y) - wd.bind_pos.y,
            ),
            deformed_size: Vec2::new(
                stretch_axis(wd.inner_bind_size.x, wd.frame_size.x, wd.min_frame_size.x, wd.bind_size.x),
                stretch_axis(wd.inner_bind_size.y, wd.frame_size.y, wd.min_frame_size.y, wd.bind_size.y),
            ),
        }
    }

    fn fire_state(&mut self, node: NodeId, name: &'static str, snap: bool) {
        self.events.push(UiEvent::State { node, name, snap });
    }

    /// `0x0062ddc0 setCaption(text, refresh)`.
    pub fn set_caption(&mut self, w: WidgetId, text: &str, refresh: bool) {
        self.widgets[w].caption = text.to_owned();
        if refresh {
            self.refresh_caption(w);
        }
    }

    /// `0x0062b920`: when the caption is non-empty, copies it into every descendant
    /// node named `caption` whose shape is a TextShape (type 3).
    pub fn refresh_caption(&mut self, w: WidgetId) {
        if self.widgets[w].caption.is_empty() {
            return;
        }
        let text: Vec<u16> = self.widgets[w].caption.encode_utf16().collect();
        let mut found = Vec::new();
        self.find_named(self.widgets[w].node, "caption", &mut found);
        for n in found {
            if self.nodes[n].text.is_some() {
                self.nodes[n].text = Some(text.clone());
                self.events.push(UiEvent::TextRebuild { node: n });
            }
        }
    }

    fn find_named(&self, n: NodeId, name: &str, out: &mut Vec<NodeId>) {
        for &c in &self.nodes[n].children {
            if self.nodes[c].name == name {
                out.push(c);
            }
            self.find_named(c, name, out);
        }
    }

    // --- virtual slots -----------------------------------------------------------------

    /// Slot 7 `update` (called when the tree is (re)loaded):
    /// Widget 0x0062a8b0, Button 0x006655d0, PopUpButton 0x0067de20, ScrollButton
    /// 0x0067d580, ScrollSlider 0x00662aa0, Edit 0x00637e80.
    pub fn update(&mut self, w: WidgetId) {
        match self.widgets[w].kind {
            WidgetKind::Edit(_) => self.edit_update(w),
            WidgetKind::Button(_) => self.button_update(w),
            WidgetKind::PopUpButton(..) => {
                // 0x0067de20
                self.button_update(w);
                let list = self.find_popup_list(w);
                if let WidgetKind::PopUpButton(_, pd) = &mut self.widgets[w].kind {
                    pd.list = list;
                }
                if let Some(l) = list {
                    let ln = self.widgets[l].node;
                    self.nodes[ln].visible = false;
                }
            }
            WidgetKind::ScrollButton(..) => {
                // 0x0067d580
                self.button_update(w);
                let target = match self.parent_widget(w) {
                    None => None,
                    Some(p) => {
                        let dir = self.scroll_button_dir(w);
                        self.find_scroll_target_btn(dir, Some(p)).or_else(|| {
                            let gp = self.parent_widget(p);
                            self.find_scroll_target_btn_up(dir, gp)
                        })
                    }
                };
                if let WidgetKind::ScrollButton(_, sd) = &mut self.widgets[w].kind {
                    sd.target = target;
                }
            }
            WidgetKind::ScrollSlider(..) => self.slider_update(w),
            _ => self.widget_update(w),
        }
    }

    /// `0x0062a8b0 Widget::update`.
    fn widget_update(&mut self, w: WidgetId) {
        let vp = self.viewport;
        let node = self.widgets[w].node;
        let wd = &mut self.widgets[w];
        wd.last_viewport = vp;
        wd.min_frame_size = Vec2::new(wd.bind_size.x - wd.inner_bind_size.x, wd.bind_size.y - wd.inner_bind_size.y);
        if !wd.enabled {
            self.fire_state(node, "widget:disable", true);
        }
        self.refresh_caption(w);
    }

    /// Slot 8 `onViewportResized` 0x0062b350: top-level widgets (no ancestor widget)
    /// receive slot 9 with offset (0, 0) and the viewport delta.
    pub fn on_viewport_resized(&mut self, w: WidgetId) {
        if self.parent_widget(w).is_none() {
            let last = self.widgets[w].last_viewport;
            let delta = Vec2::new((self.viewport.x - last.x) as f32, (self.viewport.y - last.y) as f32);
            self.on_parent_resized(w, Vec2::ZERO, delta);
        }
        self.widgets[w].last_viewport = self.viewport;
    }

    /// Slot 9 `onParentResized(offset, delta)`: Widget 0x0062af10; ScrollSlider
    /// 0x00662db0 adds a thumb re-sync.
    pub fn on_parent_resized(&mut self, w: WidgetId, offset: Vec2, delta: Vec2) {
        self.widget_on_parent_resized(w, offset, delta);
        if matches!(self.widgets[w].kind, WidgetKind::ScrollSlider(..)) {
            self.slider_sync_thumb(w);
        }
    }

    /// `0x0062af10`: anchoring then stretching.
    fn widget_on_parent_resized(&mut self, w: WidgetId, offset: Vec2, delta: Vec2) {
        let node = self.widgets[w].node;
        if self.nodes[node].parent.is_none() {
            return;
        }
        let (flags, h, v) = {
            let wd = &self.widgets[w];
            (wd.flags, wd.h_align, wd.v_align)
        };
        // Horizontal anchor: 1 = right edge (unless stretching), 2 = centre.
        if h == 1 && flags & flags::STRETCH_X == 0 {
            let pos = self.get_position(w);
            self.set_position(w, Vec2::new(offset.x + pos.x + delta.x, pos.y), true);
        } else if h == 2 {
            let pos = self.get_position(w);
            self.set_position(w, Vec2::new(delta.x * 0.5 + pos.x, pos.y), true);
        }
        // Vertical anchor.
        if v == 1 && flags & flags::STRETCH_Y == 0 {
            let pos = self.get_position(w);
            self.set_position(w, Vec2::new(pos.x, offset.y + pos.y + delta.y), true);
        } else if v == 2 {
            let pos = self.get_position(w);
            self.set_position(w, Vec2::new(pos.x, delta.y * 0.5 + pos.y), true);
        }
        // Stretch: the raw frame size (+0x70) through the world linear part.
        let size = self.widget_world(w).matrix2 * self.widgets[w].frame_size;
        if flags & (flags::STRETCH_X | flags::STRETCH_Y) != 0 {
            let mut s = size;
            if flags & flags::STRETCH_X != 0 {
                s.x = size.x + delta.x;
            }
            if flags & flags::STRETCH_Y != 0 {
                s.y = delta.y + size.y;
            }
            self.set_size(w, s, true);
        }
        let (hs, vs) = (self.widgets[w].h_slider, self.widgets[w].v_slider);
        if let Some(s) = hs {
            self.slider_sync_thumb(s);
        }
        if let Some(s) = vs {
            self.slider_sync_thumb(s);
        }
        if hs.is_some() || vs.is_some() {
            self.clamp_scroll(w);
        }
    }

    /// Slot 10 `layout` (Widget: `ret`). Game overrides: AdaptionWidget 0x00411410,
    /// BlueprintPreviewWidget 0x004348f0, CharacterStyleWidget 0x0042bb00,
    /// EnchantWidget 0x00450b90, InventoryWidget 0x004c5d50 (geometry only),
    /// OptionsWidget 0x004d46d0.
    pub fn layout(&mut self, w: WidgetId) {
        let WidgetKind::Game(g) = &self.widgets[w].kind else { return };
        let g = g.clone();
        match g.class {
            GameWidgetClass::Adaption => {
                if let Some(c) = self.gchild(&g, 0x170) {
                    let x = (self.local_size(w).x - self.width(c)) * 0.5;
                    self.set_position_xy(c, x, 50.0, true);
                }
                if let Some(c) = self.gchild(&g, 0x174) {
                    let x = self.local_size(w).x * 0.5 - 10.0;
                    self.set_position_xy(c, x, 230.0, true);
                }
            }
            GameWidgetClass::Enchant => {
                if let Some(c) = self.gchild(&g, 0x170) {
                    let x = (self.local_size(w).x - self.width(c)) * 0.5;
                    self.set_position_xy(c, x, 50.0, true);
                }
            }
            GameWidgetClass::BlueprintPreview => {
                if let Some(&n) = g.children.get(&0x298) {
                    // 0x004348f0 compares the preview node's parent with this widget's
                    // node's *parent* and then attaches it under this widget's node
                    // (0x00635fe0), so the test is true on every call after the first
                    // and the node is re-appended (moved to the top) each layout.
                    let own = self.widgets[w].node;
                    if self.nodes[n].parent != self.nodes[own].parent {
                        self.reparent(n, own);
                    }
                    if let Some(c) = self.nodes[n].widget {
                        let x = (self.width(w) - self.width(c)) * 0.5;
                        self.set_position_xy(c, x, 90.0, true);
                    }
                }
                if let Some(c) = self.gchild(&g, 0x29c) {
                    let y = (self.height(w) - self.height(c)) - 20.0;
                    let x = (self.width(w) - self.width(c)) - 20.0;
                    self.set_position_xy(c, x, y, true);
                }
                if let (Some(a), Some(b)) = (self.gchild(&g, 0x2a0), self.gchild(&g, 0x29c)) {
                    let hb = self.height(b);
                    let sw = ((self.width(w) - self.width(b)) - 40.0) - 10.0;
                    let y = (self.height(w) - self.height(b)) - 20.0;
                    self.set_rect(a, 20.0, y, sw, hb, true);
                }
            }
            GameWidgetClass::CharacterStyle => {
                // Label/control pairs by member offset, rows 30 px apart.
                for (label, control, y) in [
                    (0x164, 0x168, 12.0),
                    (0x174, 0x178, 42.0),
                    (0x16c, 0x170, 72.0),
                    (0x17c, 0x180, 102.0),
                    (0x184, 0x188, 132.0),
                ] {
                    self.layout_row(&g, w, label, control, 100.0, y);
                }
            }
            GameWidgetClass::Options => {
                let mut y = 12.0f32;
                let mut off = 0x170;
                for _ in 0..11 {
                    self.layout_row(&g, w, off, off + 4, 240.0, y);
                    off += 8;
                    y += 30.0;
                }
                // Bottom buttons: left, centred in the remaining width, right.
                if let Some(c) = self.gchild(&g, 0x1c8) {
                    let y = (self.height(w) - self.height(c)) - 20.0;
                    self.set_position_xy(c, 20.0, y, true);
                }
                if let Some(c) = self.gchild(&g, 0x1cc) {
                    let ww = self.width(w);
                    let y = (self.height(w) - self.height(c)) - 20.0;
                    let x = ((ww - 40.0) - self.width(c)) * 0.5 + 20.0;
                    self.set_position_xy(c, x, y, true);
                }
                if let Some(c) = self.gchild(&g, 0x1d0) {
                    let ww = self.width(w);
                    let y = (self.height(w) - self.height(c)) - 20.0;
                    let x = (ww - 20.0) - self.width(c);
                    self.set_position_xy(c, x, y, true);
                }
            }
            GameWidgetClass::Inventory => self.inventory_layout(w, &g),
            _ => {}
        }
    }

    fn gchild(&self, g: &GameWidget, off: u32) -> Option<WidgetId> {
        g.children.get(&off).and_then(|&n| self.nodes[n].widget)
    }

    fn layout_row(&mut self, g: &GameWidget, w: WidgetId, label: u32, control: u32, x: f32, y: f32) {
        if let Some(l) = g.children.get(&label).and_then(|&n| self.nodes[n].widget) {
            self.set_position_xy(l, x, y, true);
        }
        if let Some(c) = g.children.get(&control).and_then(|&n| self.nodes[n].widget) {
            let cx = (self.width(w) - self.width(c)) - 15.0;
            self.set_position_xy(c, cx, y, true);
        }
    }

    /// Geometry of `InventoryWidget::layout` 0x004c5d50. The original also deletes and
    /// re-clones the cell nodes from the template at +0x168 (0x006504e0, 0x00636040);
    /// here the caller creates the cells from [`inventory_grid`] and this positions the
    /// selection marker (+0x16c) and the two corner children (+0x170, +0x174).
    fn inventory_layout(&mut self, w: WidgetId, g: &GameWidget) {
        if !g.children.contains_key(&0x168) {
            return;
        }
        let (cols, _rows) = inventory_grid(self.width(w), self.height(w), g.cell_size);
        if let Some(&sel) = g.children.get(&0x16c) {
            if (g.tab as usize) < g.tab_rows.len() && g.tab >= 0 {
                self.nodes[sel].visible = true;
                if let Some(sw) = self.nodes[sel].widget {
                    self.set_position_xy(sw, 10.0, 10.0, true);
                    let i = g.selected.wrapping_sub(g.tab_rows[g.tab as usize].wrapping_mul(cols));
                    if cols != 0 {
                        let p = inventory_cell_pos(g.cell_size, i % cols, i / cols);
                        self.set_position(sw, p, true);
                    }
                }
            } else {
                self.nodes[sel].visible = false;
            }
        }
        if let Some(c) = g.children.get(&0x170).and_then(|&n| self.nodes[n].widget) {
            let x = self.width(w) - 30.0;
            self.set_position_xy(c, x, 10.0, true);
        }
        if let Some(c) = g.children.get(&0x174).and_then(|&n| self.nodes[n].widget) {
            let y = self.height(w) - 30.0;
            let x = self.width(w) - 30.0;
            self.set_position_xy(c, x, y, true);
        }
    }

    /// `Node::addChild` 0x00630be0 (as the `.plx` reader links `Node.child` lists):
    /// detaches `child` from its parent, if any, and appends it (topmost) under `parent`.
    pub fn add_child(&mut self, parent: NodeId, child: NodeId) {
        self.reparent(child, parent);
    }

    fn reparent(&mut self, n: NodeId, new_parent: NodeId) {
        if let Some(p) = self.nodes[n].parent {
            self.nodes[p].children.retain(|&c| c != n);
        }
        self.nodes[n].parent = Some(new_parent);
        self.nodes[new_parent].children.push(n);
    }

    /// Slot 5 `containsPoint(p)` (point in node-local space): Widget 0x00687d10
    /// (false), Edit 0x00637c50 (true), game widgets 0x004c59a0 (`0 ≤ p < size`).
    pub fn contains_point(&self, w: WidgetId, p: Vec2) -> bool {
        match &self.widgets[w].kind {
            WidgetKind::Edit(_) => true,
            WidgetKind::Game(g) if g.class.uses_rect_contains() => {
                // 0x004c59a0
                if 0.0 <= p.x && 0.0 <= p.y {
                    let wd = self.width(w);
                    if p.x < wd {
                        let ht = self.height(w);
                        if p.y < ht {
                            return true;
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Slot 3: Widget returns (0, 0) (0x00687cf0); ChatWidget 0x00439660 returns
    /// `getSize()` (its content size).
    pub fn content_size(&self, w: WidgetId) -> Vec2 {
        match &self.widgets[w].kind {
            WidgetKind::Game(g) if g.class == GameWidgetClass::Chat => self.get_size(w),
            _ => Vec2::ZERO,
        }
    }

    /// Slot 4 (meaning unresolved): false except ChatWidget (0x0043a000, true).
    pub fn slot4(&self, w: WidgetId) -> bool {
        matches!(&self.widgets[w].kind, WidgetKind::Game(g) if g.class == GameWidgetClass::Chat)
    }

    /// Slot 11 left press: Widget 0x0062a9e0 (edge-resize grab), Button 0x006659e0,
    /// PopUpButton 0x0067de60, ScrollButton 0x0067d5d0, ScrollSlider 0x00662b10,
    /// Edit 0x00638370.
    pub fn on_mouse_press(&mut self, w: WidgetId, metrics: Option<&dyn TextMetrics>) {
        match self.widgets[w].kind {
            WidgetKind::Button(_) => self.button_press(w),
            WidgetKind::PopUpButton(..) => {
                self.button_press(w);
                let (checked, list) = self.popup_state(w);
                if checked {
                    if let Some(l) = list {
                        self.set_popup(Some(l), Some(w));
                    }
                }
            }
            WidgetKind::ScrollButton(..) => self.scroll_button_press(w),
            WidgetKind::ScrollSlider(..) => {
                self.button_press(w);
                // FUN_00631db0(1): unresolved vector (node world position?).
                let origin = self.node_world(self.widgets[w].node).translation;
                if let WidgetKind::ScrollSlider(_, sd) = &mut self.widgets[w].kind {
                    sd.press_origin = origin;
                }
                self.slider_sync_thumb(w);
            }
            WidgetKind::Edit(_) => self.edit_press(w, metrics),
            _ => self.widget_press(w),
        }
    }

    /// `0x0062a9e0`: when resizable, grabs the edge under the cursor (outside the
    /// deformed inner rect) and captures the mouse.
    fn widget_press(&mut self, w: WidgetId) {
        if self.widgets[w].flags & flags::RESIZABLE == 0 {
            return;
        }
        self.widgets[w].resize_edge = [0, 0];
        let origin = self.node_world(self.widgets[w].node).translation;
        let cx = self.cursor.x - origin.x;
        let cy = self.cursor.y - origin.y;
        let node = self.widgets[w].node;
        let wd = &self.widgets[w];
        let left = (wd.frame_pos.x + wd.inner_bind_pos.x) - wd.bind_pos.x;
        let sx = stretch_axis(wd.inner_bind_size.x, wd.frame_size.x, wd.min_frame_size.x, wd.bind_size.x);
        let top = (wd.frame_pos.y + wd.inner_bind_pos.y) - wd.bind_pos.y;
        let sy = stretch_axis(wd.inner_bind_size.y, wd.frame_size.y, wd.min_frame_size.y, wd.bind_size.y);
        let mut edge = [0i8; 2];
        let mut capture = false;
        if cx < left {
            capture = true;
            edge[0] = -1;
        }
        if sx + left < cx {
            capture = true;
            edge[0] = 1;
        }
        if cy < top {
            capture = true;
            edge[1] = -1;
        }
        if sy + top < cy {
            capture = true;
            edge[1] = 1;
        }
        if capture {
            self.captured = Some(node);
        }
        self.widgets[w].resize_edge = edge;
    }

    /// Slot 12 left release: Widget 0x0062abe0, Button 0x00665ac0, PopUpButton
    /// 0x0067de90, Edit 0x006384f0.
    pub fn on_mouse_release(&mut self, w: WidgetId) {
        match self.widgets[w].kind {
            WidgetKind::Button(_) | WidgetKind::ScrollButton(..) | WidgetKind::ScrollSlider(..) => {
                self.button_release(w)
            }
            WidgetKind::PopUpButton(..) => {
                self.button_release(w);
                let (checked, list) = self.popup_state(w);
                if checked {
                    // 0x00653360(list, 0): re-shown without an owner (keeps it open).
                    self.set_popup(list, None);
                }
            }
            WidgetKind::Edit(_) => self.release_capture(),
            _ => {
                // 0x0062abe0: release when the captured node's handler is this widget.
                if let Some(c) = self.captured {
                    if self.owner(c) == Some(w) {
                        self.release_capture();
                    }
                }
                self.widgets[w].resize_edge = [0, 0];
            }
        }
    }

    /// Slot 13 double click: Button 0x006659d0 and Edit 0x00638320; `ret` otherwise.
    pub fn on_mouse_double_click(&mut self, w: WidgetId, metrics: Option<&dyn TextMetrics>) {
        match self.widgets[w].kind {
            WidgetKind::Button(_)
            | WidgetKind::PopUpButton(..)
            | WidgetKind::ScrollButton(..)
            | WidgetKind::ScrollSlider(..) => self.on_mouse_press(w, metrics),
            WidgetKind::Edit(_) => {
                // 0x00638320: select all.
                if let Some(len) = self.edit_len(w) {
                    if let WidgetKind::Edit(e) = &mut self.widgets[w].kind {
                        e.caret = len;
                        e.selection = -len;
                    }
                    self.events.push(UiEvent::CaretChanged { widget: w });
                }
            }
            _ => {}
        }
    }

    /// Slot 21 mouse move over (or captured by) the widget: Widget 0x0062ac20,
    /// ScrollSlider 0x00662b80, Edit 0x00638500.
    pub fn on_mouse_move(&mut self, w: WidgetId, metrics: Option<&dyn TextMetrics>) {
        match self.widgets[w].kind {
            WidgetKind::ScrollSlider(..) => self.slider_drag(w),
            WidgetKind::Edit(_) => self.edit_drag(w, metrics),
            _ => self.widget_drag(w),
        }
    }

    /// `0x0062ac20`: edge resize while the left button is down, then drag-move.
    fn widget_drag(&mut self, w: WidgetId) {
        let wf = self.widgets[w].flags;
        if wf & flags::RESIZABLE != 0 && self.left_down && self.widgets[w].resize_edge != [0, 0] {
            let mut pos = self.get_position(w);
            let mut size = self.get_size(w);
            let inv = self.node_world(self.widgets[w].node).matrix2.inverse();
            for i in 0..2 {
                let edge = self.widgets[w].resize_edge[i];
                let d = inv * (self.cursor - self.prev_cursor);
                if edge == -1 {
                    pos[i] = d[i] + pos[i];
                    size[i] -= d[i];
                } else if edge == 1 {
                    size[i] = d[i] + size[i];
                }
            }
            self.set_position(w, pos, false);
            self.set_size(w, size, true);
        }
        if wf & (flags::MOVABLE_X | flags::MOVABLE_Y) != 0 && self.drag_move_active {
            let cur = self.cursor_in_parent(w);
            let wd = &self.widgets[w];
            let mut dx = cur.x - wd.drag_start_cursor.x;
            let mut dy = cur.y - wd.drag_start_cursor.y;
            if wf & flags::MOVABLE_X == 0 {
                dx = 0.0;
            }
            if wf & flags::MOVABLE_Y == 0 {
                dy = 0.0;
            }
            let t = Vec2::new(wd.drag_start_translation.x + dx, wd.drag_start_translation.y + dy);
            let node = wd.node;
            self.nodes[node].translation = t;
            self.clamp_scroll(w);
            self.fire_node(node, event::SCROLLED);
        }
    }

    /// `0x0062b430`: the cursor in the local space of the node's parent (raw cursor for
    /// a root node).
    fn cursor_in_parent(&self, w: WidgetId) -> Vec2 {
        match self.nodes[self.widgets[w].node].parent {
            Some(p) => self.node_world(p).inverse().transform_point2(self.cursor),
            None => self.cursor,
        }
    }

    /// Slot 22 mouse enter: Button 0x00665b00; `ret` otherwise.
    pub fn on_mouse_enter(&mut self, w: WidgetId) {
        if let Some(b) = self.button(w) {
            let name = if b.button_type != 0 && b.checked { button_state::ENTER_CHECKED } else { button_state::ENTER };
            let node = self.widgets[w].node;
            self.fire_state(node, name, false);
        }
    }

    /// Slot 23 mouse leave: Button 0x00665b30; `ret` otherwise.
    pub fn on_mouse_leave(&mut self, w: WidgetId) {
        if let Some(b) = self.button(w) {
            let name = if b.button_type != 0 && b.checked { button_state::LEAVE_CHECKED } else { button_state::LEAVE };
            let node = self.widgets[w].node;
            self.fire_state(node, name, false);
        }
    }

    /// Slot 24 key down: Edit 0x006380c0; `ret 4` otherwise.
    pub fn on_key_down(&mut self, w: WidgetId, key: u16) {
        if matches!(self.widgets[w].kind, WidgetKind::Edit(_)) {
            self.edit_key_down(w, key);
        }
    }

    /// Slot 25 key up: `ret 4` for every class.
    pub fn on_key_up(&mut self, _w: WidgetId, _key: u16) {}

    /// Slot 26 char: Edit 0x00637f00; `ret 4` otherwise.
    pub fn on_char(&mut self, w: WidgetId, c: u16) {
        if matches!(self.widgets[w].kind, WidgetKind::Edit(_)) {
            self.edit_char(w, c);
        }
    }

    /// Slot 27 tick (called every frame by the engine): Button 0x00665b60,
    /// ScrollButton 0x0067d700; `ret` otherwise.
    pub fn tick(&mut self, w: WidgetId) {
        match self.widgets[w].kind {
            WidgetKind::Button(_) | WidgetKind::PopUpButton(..) | WidgetKind::ScrollSlider(..) => self.button_tick(w),
            WidgetKind::ScrollButton(..) => self.scroll_button_tick(w),
            _ => {}
        }
    }

    /// Slot 30, broadcast to every widget on left release (0x00652940 via 0x0064e236):
    /// PopUpButton 0x0067dd70; `ret` otherwise.
    pub fn on_global_left_release(&mut self, w: WidgetId) {
        if !matches!(self.widgets[w].kind, WidgetKind::PopUpButton(..)) {
            return;
        }
        let (checked, list) = self.popup_state(w);
        if checked {
            if let Some(l) = list {
                let visible = self.nodes[self.widgets[l].node].visible;
                if visible != checked {
                    self.button_set_checked(w, visible, false);
                }
            }
        }
        let adopt = matches!(&self.widgets[w].kind, WidgetKind::PopUpButton(_, pd) if pd.adopt_caption);
        if let (Some(l), true, Some(dw)) = (list, adopt, self.drag_widget) {
            let ln = self.widgets[l].node;
            if self.is_ancestor_or_self(ln, Some(self.widgets[dw].node)) {
                let cap = self.widgets[dw].caption.clone();
                self.set_caption(w, &cap, true);
            }
        }
    }

    /// Slot 35 `onDragBegin` 0x0062a690. No caller exists in Cube.exe (no call through
    /// vtable offset 0x8c/0x90 anywhere, and the engine's broadcast thunks at 0x0064e22c
    /// cover slots 28, 29, 30, 32, 33 and 38 only), so this and slot 36 are dead code in
    /// the shipped client, like [`Gui::drag_move_active`]. When movable and the
    /// parent widget is under the cursor, captures the mouse and records the cursor and
    /// translation.
    pub fn on_drag_begin(&mut self, w: WidgetId) {
        if self.widgets[w].flags & (flags::MOVABLE_X | flags::MOVABLE_Y) == 0 {
            return;
        }
        let Some(p) = self.parent_widget(w) else { return };
        if self.widget_under_cursor(p, self.widgets[p].node) {
            let node = self.widgets[w].node;
            self.captured = Some(node);
            let c = self.cursor_in_parent(w);
            let t = self.nodes[node].translation;
            let wd = &mut self.widgets[w];
            wd.drag_start_cursor = c;
            wd.drag_start_translation = t;
        }
    }

    /// Slot 36 `onDragEnd` 0x0062a780.
    pub fn on_drag_end(&mut self, w: WidgetId) {
        if self.widgets[w].flags & (flags::MOVABLE_X | flags::MOVABLE_Y) != 0
            && self.captured == Some(self.widgets[w].node)
        {
            self.release_capture();
        }
    }

    /// Slot 39 `onMouseWheel` 0x0062a7b0: scrolls y by `wheel × 10` when movable in y
    /// and the parent widget is under the cursor.
    pub fn on_mouse_wheel(&mut self, w: WidgetId) {
        if self.widgets[w].flags & flags::MOVABLE_Y == 0 {
            return;
        }
        let Some(p) = self.parent_widget(w) else { return };
        if self.widget_under_cursor(p, self.widgets[p].node) {
            let pos = self.get_position(w);
            let y = (self.wheel * 10) as f32 + pos.y;
            self.set_position(w, Vec2::new(pos.x, y), true);
            self.clamp_scroll(w);
            self.fire_node(self.widgets[w].node, event::SCROLLED);
        }
    }

    /// Slot 40 `clone`: every class copies itself (Widget 0x00627dc0 → copy ctor
    /// 0x00627450). Here: a new widget with the same data on `node`.
    pub fn clone_widget(&mut self, w: WidgetId, node: NodeId) -> WidgetId {
        let mut copy = self.widgets[w].clone();
        copy.node = node;
        let id = self.widgets.len();
        self.widgets.push(copy);
        self.nodes[node].widget = Some(id);
        id
    }

    /// Slot 41 `setEnabled` 0x00628f40.
    pub fn set_enabled(&mut self, w: WidgetId, enabled: bool) {
        self.widgets[w].enabled = enabled;
        let node = self.widgets[w].node;
        self.fire_state(node, if enabled { "widget:enable" } else { "widget:disable" }, true);
    }

    /// Slot 42, called after scrolling (0x006278a0, sliders, scroll buttons): `ret` in
    /// every class ported here.
    pub fn on_scrolled(&mut self, _w: WidgetId) {}

    /// Slot 1, the game widgets' per-frame body. Not ported (game logic, cw-client);
    /// see [`GameWidgetClass::frame_update_summary`]. Edit's slot 1 0x006378e0 draws
    /// the selection (drawInvertedRect) and caret (drawCaret) and is a render concern.
    pub fn frame_update(&mut self, _w: WidgetId) {}

    // --- scroll helpers ----------------------------------------------------------------

    /// `0x006278a0`: clamps a scrolled content widget inside its parent widget:
    /// position ≤ 0, ≥ parent − content when larger, 0 when smaller; then re-syncs the
    /// sliders and calls slot 42.
    pub fn clamp_scroll(&mut self, w: WidgetId) {
        let Some(p) = self.parent_widget(w) else { return };
        let mut pos = self.get_position(w);
        for axis in 0..2 {
            if 0.0 < pos[axis] {
                pos[axis] = 0.0;
            }
            let this = self.get_size(w)[axis];
            let parent = self.get_size(p)[axis];
            if parent < this && pos[axis] + this < parent {
                pos[axis] = parent - this;
            }
        }
        for axis in 0..2 {
            let this = self.get_size(w)[axis];
            let parent = self.get_size(p)[axis];
            if this < parent {
                pos[axis] = 0.0;
            }
        }
        self.set_position(w, pos, true);
        let (hs, vs) = (self.widgets[w].h_slider, self.widgets[w].v_slider);
        if let Some(s) = hs {
            self.slider_sync_thumb(s);
        }
        if let Some(s) = vs {
            self.slider_sync_thumb(s);
        }
        self.on_scrolled(w);
    }

    /// `0x0062f5d0`: re-syncs both sliders of a content widget.
    pub fn sync_scroll_bars(&mut self, w: WidgetId) {
        if let Some(s) = self.widgets[w].h_slider {
            self.slider_sync_thumb(s);
        }
        if let Some(s) = self.widgets[w].v_slider {
            self.slider_sync_thumb(s);
        }
    }

    fn slider_axis(&self, w: WidgetId) -> usize {
        match &self.widgets[w].kind {
            WidgetKind::ScrollSlider(_, sd) => usize::from(sd.direction != 1),
            _ => 1,
        }
    }

    fn slider_target(&self, w: WidgetId) -> Option<WidgetId> {
        match &self.widgets[w].kind {
            WidgetKind::ScrollSlider(_, sd) => sd.target,
            _ => None,
        }
    }

    /// `0x00662aa0 ScrollSlider::update`.
    fn slider_update(&mut self, w: WidgetId) {
        self.button_update(w);
        let dir = match &self.widgets[w].kind {
            WidgetKind::ScrollSlider(_, sd) => sd.direction,
            _ => 0,
        };
        let target = match self.parent_widget(w) {
            None => None,
            Some(p) => self.find_scroll_target(dir, Some(p)).or_else(|| {
                let gp = self.parent_widget(p);
                self.find_scroll_target_up(dir, gp)
            }),
        };
        if let WidgetKind::ScrollSlider(_, sd) = &mut self.widgets[w].kind {
            sd.target = target;
        }
        if let Some(t) = target {
            // 0x0062de40 / 0x0062ddf0 (+ 0x00662fb0 storing the target again).
            if dir == 0 {
                self.widgets[t].v_slider = Some(w);
            } else {
                self.widgets[t].h_slider = Some(w);
            }
        }
        self.slider_sync_thumb(w);
    }

    /// `0x00662e10`: first widget at or below `w` movable along the slider's axis.
    fn find_scroll_target(&self, dir: i32, w: Option<WidgetId>) -> Option<WidgetId> {
        let w = w?;
        let f = self.widgets[w].flags;
        if (dir == 0 && f & flags::MOVABLE_Y != 0) || (dir == 1 && f & flags::MOVABLE_X != 0) {
            return Some(w);
        }
        self.child_widgets(w).into_iter().find_map(|c| self.find_scroll_target(dir, Some(c)))
    }

    /// `0x00662dd0`: [`Gui::find_scroll_target`] walking up the ancestors.
    fn find_scroll_target_up(&self, dir: i32, mut w: Option<WidgetId>) -> Option<WidgetId> {
        while let Some(c) = w {
            if let Some(t) = self.find_scroll_target(dir, Some(c)) {
                return Some(t);
            }
            w = self.parent_widget(c);
        }
        None
    }

    /// `0x0067d8e0`: ScrollButton's target search (dirs 0/1 need movable y, 2/3 x).
    fn find_scroll_target_btn(&self, dir: i32, w: Option<WidgetId>) -> Option<WidgetId> {
        let w = w?;
        let f = self.widgets[w].flags;
        if ((dir == 0 || dir == 1) && f & flags::MOVABLE_Y != 0) || ((dir == 2 || dir == 3) && f & flags::MOVABLE_X != 0) {
            return Some(w);
        }
        self.child_widgets(w).into_iter().find_map(|c| self.find_scroll_target_btn(dir, Some(c)))
    }

    /// `0x0067d8a0` (by analogy with 0x00662dd0).
    fn find_scroll_target_btn_up(&self, dir: i32, mut w: Option<WidgetId>) -> Option<WidgetId> {
        while let Some(c) = w {
            if let Some(t) = self.find_scroll_target_btn(dir, Some(c)) {
                return Some(t);
            }
            w = self.parent_widget(c);
        }
        None
    }

    /// `0x00662860`: sizes the thumb to `track / max(content / viewport, 1)` and places
    /// it at the content's scroll fraction.
    pub fn slider_sync_thumb(&mut self, s: WidgetId) {
        let Some(t) = self.slider_target(s) else { return };
        let Some(track) = self.parent_widget(s) else { return };
        let Some(view) = self.parent_widget(t) else { return };
        let a = self.slider_axis(s);
        let mut ratio = self.get_size(t)[a] / self.get_size(view)[a];
        if 1.0 > ratio {
            ratio = 1.0;
        }
        let mut size = self.get_size(s);
        size[a] = self.get_size(track)[a] / ratio;
        self.set_size(s, size, true);
        let content = self.get_size(t)[a];
        let viewport = self.get_size(view)[a];
        // `jc` after comiss(viewport, content): taken when viewport < content.
        let frac = if viewport < content {
            -(self.get_position(t)[a] / (content - viewport))
        } else {
            0.0
        };
        self.slider_set_fraction(s, frac);
    }

    /// `0x00662f00`: thumb position = `(track − thumb) × clamp(t, 0, 1)` on its axis.
    pub fn slider_set_fraction(&mut self, s: WidgetId, t: f32) {
        let Some(track) = self.parent_widget(s) else { return };
        let mut t = t;
        if t < 0.0 {
            t = 0.0;
        } else if 1.0 < t {
            t = 1.0;
        }
        let a = self.slider_axis(s);
        let mut pos = self.get_position(s);
        pos[a] = (self.get_size(track)[a] - self.get_size(s)[a]) * t;
        self.set_position(s, pos, true);
    }

    /// `0x00662b80 ScrollSlider::onMouseDrag`.
    fn slider_drag(&mut self, s: WidgetId) {
        self.widget_drag(s);
        let Some(track) = self.parent_widget(s) else { return };
        if !self.left_down {
            return;
        }
        let a = self.slider_axis(s);
        let max = self.get_size(track)[a] - self.get_size(s)[a];
        let mut pos = self.get_position(s);
        let d = self.cursor - self.prev_cursor;
        pos[a] = d[a] + pos[a];
        if pos[a] < 0.0 {
            pos[a] = 0.0;
        }
        if max < pos[a] {
            pos[a] = max;
        }
        self.set_position(s, pos, true);
        let Some(t) = self.slider_target(s) else { return };
        let Some(view) = self.parent_widget(t) else { return };
        let content = self.get_size(t)[a];
        let viewport = self.get_size(view)[a];
        if viewport < content {
            let mut tp = self.get_position(t);
            let sp = self.get_position(s)[a];
            tp[a] = (sp / (self.get_size(track)[a] - self.get_size(s)[a])) * (viewport - content);
            self.set_position(t, tp, true);
        }
        self.clamp_scroll(t);
        self.on_scrolled(t);
        self.fire_node(self.widgets[t].node, event::SCROLLED);
    }

    fn scroll_button_dir(&self, w: WidgetId) -> i32 {
        match &self.widgets[w].kind {
            WidgetKind::ScrollButton(_, sd) => sd.direction,
            _ => -1,
        }
    }

    fn scroll_button_step(&mut self, w: WidgetId, step: f32) {
        let WidgetKind::ScrollButton(_, sd) = &self.widgets[w].kind else { return };
        let (dir, Some(t)) = (sd.direction, sd.target) else { return };
        let pos = self.get_position(t);
        let p = match dir {
            0 => Some(Vec2::new(pos.x, pos.y + step)),
            1 => Some(Vec2::new(pos.x, pos.y - step)),
            2 => Some(Vec2::new(pos.x + step, pos.y)),
            3 => Some(Vec2::new(pos.x - step, pos.y)),
            _ => None,
        };
        if let Some(p) = p {
            self.set_position(t, p, true);
        }
        self.clamp_scroll(t);
        self.sync_scroll_bars(t);
    }

    /// `0x0067d5d0 ScrollButton::onMousePress`: one 20-unit step.
    fn scroll_button_press(&mut self, w: WidgetId) {
        self.button_press(w);
        let target = match &self.widgets[w].kind {
            WidgetKind::ScrollButton(_, sd) => sd.target,
            _ => None,
        };
        let Some(t) = target else { return };
        self.scroll_button_step(w, 20.0);
        if let WidgetKind::ScrollButton(_, sd) = &mut self.widgets[w].kind {
            sd.counter = 0;
        }
        self.on_scrolled(t);
        self.fire_node(self.widgets[t].node, event::SCROLLED);
    }

    /// `0x0067d700 ScrollButton::tick`: while pressed, `(frameMs × 20) × 0.01` per frame.
    fn scroll_button_tick(&mut self, w: WidgetId) {
        let target = match &self.widgets[w].kind {
            WidgetKind::ScrollButton(b, sd) if b.pressed => sd.target,
            _ => None,
        };
        let Some(t) = target else { return };
        let step = (self.frame_ms.wrapping_mul(20)) as f32 * 0.01;
        self.scroll_button_step(w, step);
        self.on_scrolled(t);
        self.fire_node(self.widgets[t].node, event::SCROLLED);
    }

    // --- Button --------------------------------------------------------------------------

    fn button(&self, w: WidgetId) -> Option<&ButtonData> {
        match &self.widgets[w].kind {
            WidgetKind::Button(b)
            | WidgetKind::PopUpButton(b, _)
            | WidgetKind::ScrollButton(b, _)
            | WidgetKind::ScrollSlider(b, _) => Some(b),
            _ => None,
        }
    }

    fn button_mut(&mut self, w: WidgetId) -> Option<&mut ButtonData> {
        match &mut self.widgets[w].kind {
            WidgetKind::Button(b)
            | WidgetKind::PopUpButton(b, _)
            | WidgetKind::ScrollButton(b, _)
            | WidgetKind::ScrollSlider(b, _) => Some(b),
            _ => None,
        }
    }

    /// `0x006655d0 Button::update`: Widget::update, rebuilds the radio group (the other
    /// radio buttons among the parent widget's child widgets) and snaps the node to its
    /// resting states.
    fn button_update(&mut self, w: WidgetId) {
        self.widget_update(w);
        let mut group = Vec::new();
        if let Some(p) = self.parent_widget(w) {
            // The node flag RADIO_SCOPE branch (collect from the grandparent too) is a
            // decompiler tangle; this keeps the plain case.
            for c in self.child_widgets(p) {
                if c != w && self.widgets[c].class_id == 1 {
                    if let Some(b) = self.button(c) {
                        if b.button_type == 2 {
                            group.push(c);
                        }
                    }
                }
            }
        }
        let node = self.widgets[w].node;
        let enabled = self.widgets[w].enabled;
        let Some(b) = self.button_mut(w) else { return };
        b.radio_group = group;
        let (bt, checked) = (b.button_type, b.checked);
        if enabled {
            if bt == 0 {
                self.fire_state(node, button_state::RELEASE, true);
            } else {
                self.fire_state(node, button_state::PRESS_CHECKED, true);
                self.fire_state(node, button_state::RELEASE_CHECKED, true);
            }
            self.fire_state(node, button_state::RELEASE, true);
            self.fire_state(node, button_state::LEAVE, true);
            if checked {
                self.fire_state(node, button_state::PRESS, true);
                self.fire_state(node, button_state::RELEASE_CHECKED, true);
                self.fire_state(node, button_state::LEAVE_CHECKED, true);
            }
        }
    }

    /// `0x006659e0 Button::onMousePress`.
    fn button_press(&mut self, w: WidgetId) {
        let node = self.widgets[w].node;
        self.captured = Some(node);
        let b = self.button_mut(w).expect("button");
        b.repeat_acc = -500;
        b.repeat_interval = 150;
        b.pressed = true;
        let (bt, group) = (b.button_type, b.radio_group.clone());
        if bt != 0 {
            if bt == 2 {
                for g in group {
                    let gn = self.widgets[g].node;
                    let was = self.button_mut(g).is_some_and(|gb| std::mem::replace(&mut gb.checked, false));
                    if was {
                        self.fire_state(gn, button_state::LEAVE, false);
                    }
                }
            }
            let checked = self.button(w).unwrap().checked;
            self.fire_state(node, if checked { button_state::PRESS_CHECKED } else { button_state::PRESS }, false);
            let b = self.button_mut(w).unwrap();
            if !b.checked || b.button_type != 2 {
                b.checked = !b.checked;
            }
            return;
        }
        self.fire_state(node, button_state::PRESS, false);
    }

    /// `0x00665ac0 Button::onMouseRelease`.
    fn button_release(&mut self, w: WidgetId) {
        self.release_capture();
        let node = self.widgets[w].node;
        let b = self.button_mut(w).expect("button");
        b.pressed = false;
        let name = if b.button_type == 0 || !b.checked { button_state::RELEASE } else { button_state::RELEASE_CHECKED };
        self.fire_state(node, name, false);
    }

    /// `0x00665b60 Button::tick`: auto-repeat. The first repeat comes 650 ms after the
    /// press (accumulator starts at −500, interval 150), then the interval shrinks by
    /// 10 ms per repeat down to 50 ms.
    fn button_tick(&mut self, w: WidgetId) {
        let frame = self.frame_ms;
        let node = self.widgets[w].node;
        let mut fires = 0;
        {
            let b = self.button_mut(w).expect("button");
            if !b.pressed {
                return;
            }
            b.repeat_acc = b.repeat_acc.wrapping_add(frame);
            while b.repeat_interval < b.repeat_acc {
                fires += 1;
                let iv = b.repeat_interval;
                b.repeat_acc -= iv;
                if 50 < iv {
                    b.repeat_interval = iv - 10;
                }
            }
        }
        for _ in 0..fires {
            self.fire_node(node, event::REPEAT);
        }
    }

    /// `0x006653a0 Button::setChecked(checked, fromGroup)`: snaps the node states.
    pub fn button_set_checked(&mut self, w: WidgetId, checked: bool, from_group: bool) {
        let node = self.widgets[w].node;
        let Some(b) = self.button(w) else { return };
        if b.checked == checked || b.button_type == 0 {
            return;
        }
        if !b.checked && b.button_type == 2 && !from_group {
            for g in b.radio_group.clone() {
                self.button_set_checked(g, false, true);
            }
        }
        self.button_mut(w).unwrap().checked = checked;
        let names: [&'static str; 4] = if !checked {
            [button_state::ENTER_CHECKED, button_state::PRESS_CHECKED, button_state::RELEASE, button_state::LEAVE]
        } else {
            [button_state::ENTER, button_state::PRESS, button_state::RELEASE_CHECKED, button_state::LEAVE_CHECKED]
        };
        for n in names {
            self.fire_state(node, n, true);
        }
    }

    fn popup_state(&self, w: WidgetId) -> (bool, Option<WidgetId>) {
        match &self.widgets[w].kind {
            WidgetKind::PopUpButton(b, pd) => (b.checked, pd.list),
            _ => (false, None),
        }
    }

    /// `0x0067dbf0`: the pop-up list: a child widget flagged [`flags::POPUP_LIST`],
    /// else one among the parent widget's children.
    fn find_popup_list(&self, w: WidgetId) -> Option<WidgetId> {
        let is_list = |c: &WidgetId| self.widgets[*c].flags & flags::POPUP_LIST != 0;
        if let Some(c) = self.child_widgets(w).into_iter().find(is_list) {
            return Some(c);
        }
        let p = self.parent_widget(w)?;
        self.child_widgets(p).into_iter().find(is_list)
    }

    // --- Edit ----------------------------------------------------------------------------

    fn edit(&self, w: WidgetId) -> &EditData {
        match &self.widgets[w].kind {
            WidgetKind::Edit(e) => e,
            _ => panic!("not an Edit"),
        }
    }

    fn edit_mut(&mut self, w: WidgetId) -> &mut EditData {
        match &mut self.widgets[w].kind {
            WidgetKind::Edit(e) => e,
            _ => panic!("not an Edit"),
        }
    }

    fn edit_len(&self, w: WidgetId) -> Option<i32> {
        let n = self.edit(w).text_node?;
        Some(self.nodes[n].text.as_ref().map_or(0, |t| t.len() as i32))
    }

    /// The Edit's current text.
    pub fn edit_text(&self, w: WidgetId) -> String {
        self.edit(w)
            .text_node
            .and_then(|n| self.nodes[n].text.as_ref())
            .map(|t| String::from_utf16_lossy(t))
            .unwrap_or_default()
    }

    /// Sets the Edit's filter (`Edit+0x164`).
    pub fn set_edit_filter(&mut self, w: WidgetId, filter: Option<EditFilter>) {
        self.edit_mut(w).filter = filter;
    }

    /// `0x00637e80 Edit::update` (does not call Widget::update).
    fn edit_update(&mut self, w: WidgetId) {
        let node = self.widgets[w].node;
        let tn = self.nodes[node].text.is_some().then_some(node);
        self.edit_mut(w).text_node = tn;
        self.edit_clamp(w);
        if let Some(p) = self.nodes[node].parent {
            if self.nodes[p].shape.is_some() {
                self.nodes[p].clip = true;
            }
        }
        let t = self.nodes[node].translation;
        self.edit_mut(w).origin_translation = t;
    }

    /// `0x00638cf0`: clamps caret and selection. Note the last rule moves the caret, not
    /// the selection.
    fn edit_clamp(&mut self, w: WidgetId) {
        let Some(len) = self.edit_len(w) else {
            let e = self.edit_mut(w);
            e.caret = 0;
            e.selection = 0;
            return;
        };
        let e = self.edit_mut(w);
        if e.caret < 0 {
            e.caret = 0;
        }
        if len < e.caret {
            e.caret = len;
        }
        let c = e.caret;
        if e.selection + c < 0 {
            e.selection = -c;
        }
        if len < e.selection + c {
            e.caret = len - e.selection;
        }
    }

    fn edit_update_caret(&mut self, w: WidgetId) {
        // 0x00638610 scrolls the text node horizontally to keep the caret visible
        // (needs glyph metrics and the parent clip rect); the caller re-runs it.
        self.events.push(UiEvent::CaretChanged { widget: w });
    }

    fn fire_widget(&mut self, w: WidgetId, ev: u32) {
        // 0x00627cb0: no enabled check.
        if self.widgets[w].connections.contains_key(&ev) {
            self.events.push(UiEvent::Signal { widget: w, event: ev });
        }
    }

    /// `0x00637850`: deletes the selection, or one character forward when there is no
    /// selection (Delete key), then fires TEXT_CHANGED.
    fn edit_delete_selection(&mut self, w: WidgetId) {
        self.edit_clamp(w);
        let Some(tn) = self.edit(w).text_node else { return };
        let len = self.nodes[tn].text.as_ref().map_or(0, |t| t.len()) as i32;
        let (caret, sel) = (self.edit(w).caret, self.edit(w).selection);
        if caret > len {
            return;
        }
        let (mut start, mut count) = (caret, sel);
        if count == 0 {
            count = 1;
        } else if count < 0 {
            start = caret + count;
            count = -count;
        }
        if let Some(t) = self.nodes[tn].text.as_mut() {
            let s = start.max(0) as usize;
            let e = (s + count as usize).min(t.len());
            if s <= t.len() {
                t.drain(s..e);
            }
        }
        let e = self.edit_mut(w);
        if e.selection < 0 {
            e.caret += e.selection;
        }
        e.selection = 0;
        self.events.push(UiEvent::TextRebuild { node: tn });
        self.fire_widget(w, event::TEXT_CHANGED);
        self.edit_update_caret(w);
    }

    /// `0x006380c0 Edit::onKeyDown`.
    fn edit_key_down(&mut self, w: WidgetId, key: u16) {
        let Some(tn) = self.edit(w).text_node else { return };
        self.edit_clamp(w);
        self.widgets[w].dirty = true;
        let shift = self.keys_down.contains(&vk::SHIFT);
        let len = self.nodes[tn].text.as_ref().map_or(0, |t| t.len()) as i32;
        match key {
            vk::RETURN => {
                self.set_focus(None);
                self.edit_update_caret(w);
            }
            vk::LEFT => {
                let e = self.edit_mut(w);
                if 0 < e.caret {
                    e.caret -= 1;
                    if shift {
                        e.selection += 1;
                    } else {
                        e.selection = 0;
                    }
                }
                self.edit_update_caret(w);
            }
            vk::RIGHT => {
                let e = self.edit_mut(w);
                if e.caret < len {
                    e.caret += 1;
                    if shift {
                        e.selection -= 1;
                    } else {
                        e.selection = 0;
                    }
                }
                self.edit_update_caret(w);
            }
            vk::HOME => {
                let e = self.edit_mut(w);
                if shift {
                    e.selection += e.caret;
                } else {
                    e.selection = 0;
                }
                e.caret = 0;
                self.edit_update_caret(w);
            }
            vk::END => {
                let e = self.edit_mut(w);
                if shift {
                    e.selection += e.caret - len;
                } else {
                    e.selection = 0;
                }
                e.caret = len;
                self.edit_update_caret(w);
            }
            vk::BACK if self.edit(w).selection == 0 => {
                let caret = self.edit(w).caret;
                if caret < 1 {
                    return;
                }
                if let Some(t) = self.nodes[tn].text.as_mut() {
                    t.remove((caret - 1) as usize);
                }
                self.edit_mut(w).caret -= 1;
                self.events.push(UiEvent::TextRebuild { node: tn });
                self.edit_update_caret(w);
                self.fire_widget(w, event::TEXT_CHANGED);
            }
            vk::BACK | vk::DELETE => {
                self.edit_delete_selection(w);
                self.edit_update_caret(w);
            }
            _ => {}
        }
    }

    /// `0x00637f00 Edit::onChar`: inserts a printable character at the caret, replacing
    /// the selection; the filter must accept the character (when replacing) and the
    /// resulting text.
    fn edit_char(&mut self, w: WidgetId, c: u16) {
        let Some(tn) = self.edit(w).text_node else { return };
        if c < 0x20 {
            return;
        }
        self.widgets[w].dirty = true;
        let filter = self.edit(w).filter;
        if self.edit(w).selection != 0 {
            match filter {
                None => self.edit_delete_selection(w),
                Some(f) => {
                    if f.accept(&[c]) {
                        self.edit_delete_selection(w);
                    } else {
                        return;
                    }
                }
            }
        }
        let mut text = self.nodes[tn].text.clone().unwrap_or_default();
        self.edit_clamp(w);
        let caret = self.edit(w).caret.max(0) as usize;
        text.insert(caret.min(text.len()), c);
        if let Some(f) = filter {
            if !f.accept(&text) {
                return;
            }
        }
        self.nodes[tn].text = Some(text);
        self.events.push(UiEvent::TextRebuild { node: tn });
        self.edit_mut(w).caret += 1;
        self.edit_update_caret(w);
        self.fire_widget(w, event::TEXT_CHANGED);
    }

    /// `0x00638b60`: caret index from a local point: the number of leading characters
    /// whose `x + 0.7 × width` is left of the point.
    fn edit_hit_caret(&mut self, w: WidgetId, p: Vec2, metrics: Option<&dyn TextMetrics>) {
        let Some(tn) = self.edit(w).text_node else { return };
        let text = self.nodes[tn].text.clone().unwrap_or_default();
        let mut caret = 0;
        if let Some(m) = metrics {
            for i in 0..text.len() {
                let (x, wdt) = m.glyph_box(&text, i);
                if wdt * 0.7 + x < p.x {
                    caret = i as i32 + 1;
                }
            }
        }
        self.edit_mut(w).caret = caret;
        self.edit_clamp(w);
        self.widgets[w].dirty = true;
    }

    /// `0x00638370 Edit::onMousePress`.
    fn edit_press(&mut self, w: WidgetId, metrics: Option<&dyn TextMetrics>) {
        let Some(tn) = self.edit(w).text_node else { return };
        let node = self.widgets[w].node;
        self.edit_mut(w).selecting = false;
        let local = self.node_world(node).inverse().transform_point2(self.cursor);
        if self.focused == Some(w) {
            if !self.keys_down.contains(&vk::SHIFT) {
                self.edit_hit_caret(w, local, metrics);
                self.edit_mut(w).selection = 0;
            } else {
                let old = self.edit(w).caret;
                self.edit_hit_caret(w, local, metrics);
                let e = self.edit_mut(w);
                // As in the original (0x0063849c: `add eax, esi; sub [sel], eax`): the
                // selection is reduced by new + old caret, not new − old.
                e.selection = e.selection.wrapping_sub(e.caret.wrapping_add(old));
            }
            self.edit_mut(w).selecting = true;
        } else {
            let len = self.nodes[tn].text.as_ref().map_or(0, |t| t.len()) as i32;
            let e = self.edit_mut(w);
            e.caret = len;
            e.selection = -len;
            self.set_focus(Some(w));
        }
        self.edit_clamp(w);
        self.edit_update_caret(w);
        self.captured = Some(node);
    }

    /// `0x00638500 Edit::onMouseDrag`: extends the selection while selecting.
    fn edit_drag(&mut self, w: WidgetId, metrics: Option<&dyn TextMetrics>) {
        if !self.left_down || self.focused != Some(w) || !self.edit(w).selecting {
            return;
        }
        let node = self.widgets[w].node;
        let local = self.node_world(node).inverse().transform_point2(self.cursor);
        let old = self.edit(w).caret;
        self.edit_hit_caret(w, local, metrics);
        let e = self.edit_mut(w);
        e.selection += old - e.caret;
        self.edit_clamp(w);
        self.edit_update_caret(w);
    }

    // --- engine: events, capture, focus, pop-up ----------------------------------------------

    /// `0x00653620`: fires `ev` on a node's owner widget unless the node ignores events.
    pub fn fire_node(&mut self, n: NodeId, ev: u32) {
        if self.nodes[n].flags & node_flags::NO_EVENTS != 0 {
            return;
        }
        let Some(w) = self.owner(n) else { return };
        if let Some(conn) = self.widgets[w].connections.get(&ev) {
            if self.widgets[w].enabled || !conn.only_when_enabled {
                self.events.push(UiEvent::Signal { widget: w, event: ev });
            }
        }
    }

    /// `0x00659cf0`: clears the capture and re-runs hover at the cursor.
    pub fn release_capture(&mut self) {
        self.captured = None;
        let c = self.cursor;
        self.inject_mouse_move(c.x, c.y, None);
    }

    /// `0x00659df0 setFocus`.
    pub fn set_focus(&mut self, w: Option<WidgetId>) {
        if let Some(f) = self.focused {
            if w == Some(f) {
                return;
            }
            self.widgets[f].dirty = true;
            let n = self.widgets[f].node;
            self.fire_node(n, event::FOCUS_LOST);
        }
        self.focused = w;
        if let Some(w) = w {
            let n = self.widgets[w].node;
            if self.nodes[n].flags & node_flags::NO_EVENTS == 0 {
                if let Some(o) = self.owner(n) {
                    if let Some(conn) = self.widgets[o].connections.get(&event::FOCUS_GAINED) {
                        if self.widgets[o].enabled || !conn.only_when_enabled {
                            self.events.push(UiEvent::Signal { widget: o, event: event::FOCUS_GAINED });
                        }
                    }
                }
            }
        }
    }

    /// `0x006531e0 hasTextFocus`: the focused widget.
    pub fn has_text_focus(&self) -> Option<WidgetId> {
        self.focused
    }

    /// `0x00653360 setPopup(widget, owner)`: hides the previous pop-up, moves the new
    /// one into the overlay layer, places it below `owner` (or keeps its position),
    /// keeps it inside the viewport (flipping above the owner when it does not fit
    /// below) and shows it.
    pub fn set_popup(&mut self, w: Option<WidgetId>, owner: Option<WidgetId>) {
        if let Some(old) = self.popup {
            let n = self.widgets[old].node;
            self.nodes[n].visible = false;
        }
        self.popup = w;
        let Some(p) = w else { return };
        if let Some(layer) = self.overlay.or(self.root) {
            let n = self.widgets[p].node;
            if self.nodes[n].parent != Some(layer) && n != layer {
                self.reparent(n, layer);
            }
        }
        if let Some(o) = owner {
            let oh = self.height(o);
            let op = self.widget_world(o).translation;
            self.set_position(p, Vec2::new(op.x + 0.0, op.y + oh), true);
        }
        let pos = self.get_position(p);
        if pos.x < 0.0 {
            let y = self.get_position(p).y;
            self.set_position_xy(p, 0.0, y, true);
        }
        let pos = self.get_position(p);
        if pos.y < 0.0 {
            let x = self.get_position(p).x;
            self.set_position_xy(p, x, 0.0, true);
        }
        let vw = self.viewport.x as f32;
        if vw < self.get_position(p).x + self.width(p) {
            let y = self.get_position(p).y;
            let x = vw - self.width(p);
            self.set_position_xy(p, x, y, true);
        }
        let vh = self.viewport.y as f32;
        if vh < self.get_position(p).y + self.height(p) {
            match owner {
                None => {
                    let y = vh - self.height(p);
                    let x = self.get_position(p).x;
                    self.set_position_xy(p, x, y, true);
                }
                Some(o) => {
                    let ph = self.height(p);
                    let op = self.widget_world(o).translation;
                    self.set_position(p, Vec2::new(op.x - 0.0, op.y - ph), true);
                }
            }
        }
        if let Some(cur) = self.popup {
            let n = self.widgets[cur].node;
            self.nodes[n].visible = true;
        }
    }

    /// `0x00636560 Node::hitTest(point, filter)`: depth-first from `n`, children in
    /// reverse order (topmost first); returns the deepest hit node.
    pub fn hit_test(&self, n: NodeId, p: Vec2, filter: u32) -> Option<NodeId> {
        let node = &self.nodes[n];
        if !node.visible || node.flags & node_flags::DISABLED != 0 || node.flags & node_flags::NO_HIT != 0 {
            return None;
        }
        if let Some(w) = node.widget {
            let wd = &self.widgets[w];
            if wd.clip_hit && self.clip_hits_to_widgets && wd.flags & flags::HIT_ALL == 0 {
                let pos = self.widget_world(w).translation;
                let size = self.get_size(w);
                if p.x < pos.x || p.y < pos.y || size.x + pos.x < p.x || size.y + pos.y < p.y {
                    return None;
                }
            }
        }
        let local = self.node_world(n).inverse().transform_point2(p);
        let mut shape_point = local;
        if let (Some(o), Some(s)) = (self.owner(n), node.shape.as_ref()) {
            if s.is_deformed() {
                shape_point = self.undeform_local(o, n, local);
            }
        }
        let hit_self = node.shape.as_ref().is_some_and(|s| s.contains_point(shape_point))
            || node.widget.is_some_and(|w| {
                self.contains_point(w, local) || self.widgets[w].flags & flags::HIT_ALL != 0
            });
        if !node.clip || hit_self {
            for &c in node.children.iter().rev() {
                if let Some(h) = self.hit_test(c, p, filter) {
                    return Some(h);
                }
            }
            if filter & 4 != 0 && node.flags & node_flags::HIT_FILTER_4 != 0 {
                return None;
            }
            if filter & 1 != 0 && node.flags & 1 != 0 {
                return None;
            }
            if filter & 2 == 0 && node.flags & node_flags::NO_EVENTS != 0 {
                return None;
            }
            return hit_self.then_some(n);
        }
        None
    }

    /// `Widget::undeformPoint` `Cube.exe 0x0062e180` (the Deformer's slot 1) on a point in the
    /// local space of node `n`: the point is taken into `w`'s bind space (node `n`'s world,
    /// the inverse of the widget node's world, the file's `bindMatrix`: the matrices
    /// 0x0062e180 multiplies from `n+0x48`, `widget node+0x88` and `widget+0x84`), the
    /// 9-slice map is run backwards ([`DeformUniforms::undeform`]) and the result is brought
    /// back into `n`'s space. The drawer does the same forwards (`0x0068ab70`,
    /// `cw_ui::render`). A widget whose deformation is the identity returns the point as it
    /// is (the early exit of 0x0062e180).
    pub fn undeform_local(&self, w: WidgetId, n: NodeId, local: Vec2) -> Vec2 {
        let du = self.deform_uniforms(w);
        if du.is_identity() {
            return local;
        }
        let wd = &self.widgets[w];
        // `bind_matrix` holds the inverse of the file's matrix (widget → node).
        let to_bind = wd.bind_matrix.inverse() * self.node_world(wd.node).inverse() * self.node_world(n);
        to_bind.inverse().transform_point2(du.undeform(to_bind.transform_point2(local)))
    }

    /// `0x00629300 Widget::isUnderCursor(node)`: like the hit test but only through
    /// nodes that belong to `w` (no widget, or `w` itself), always un-deforming with
    /// `w`'s deformer.
    pub fn widget_under_cursor(&self, w: WidgetId, n: NodeId) -> bool {
        let node = &self.nodes[n];
        if node.flags & node_flags::DISABLED != 0 || !node.visible {
            return false;
        }
        if node.widget.is_some_and(|nw| nw != w) {
            return false;
        }
        let local = self.node_world(n).inverse().transform_point2(self.cursor);
        let und = self.undeform_local(w, n, local);
        if node.shape.as_ref().is_some_and(|s| s.contains_point(und)) {
            return true;
        }
        if node.widget.is_some_and(|nw| self.contains_point(nw, local)) {
            return true;
        }
        if !node.clip {
            for &c in node.children.iter().rev() {
                if self.widget_under_cursor(w, c) {
                    return true;
                }
            }
        }
        false
    }

    /// `0x00650ae0`: the captured node, else the node under the cursor (`0x00636560` from
    /// the root).
    pub fn current_target(&self) -> Option<NodeId> {
        self.captured.or_else(|| self.root.and_then(|r| self.hit_test(r, self.cursor, 0)))
    }

    /// `0x0064ef70`: pre-order over visible, enabled, event-receiving nodes, calling
    /// `f` on each node's own widget.
    pub fn broadcast(&mut self, f: &mut dyn FnMut(&mut Gui, WidgetId)) {
        if let Some(r) = self.root {
            self.broadcast_from(r, f);
        }
    }

    fn broadcast_from(&mut self, n: NodeId, f: &mut dyn FnMut(&mut Gui, WidgetId)) {
        let node = &self.nodes[n];
        if !node.visible || node.flags & (node_flags::DISABLED | node_flags::NO_EVENTS) != 0 {
            return;
        }
        if let Some(w) = node.widget {
            f(self, w);
        }
        let children = self.nodes[n].children.clone();
        for c in children {
            self.broadcast_from(c, f);
        }
    }

    fn fire_mouse(&mut self, n: NodeId, ev: u32) {
        if self.nodes[n].flags & node_flags::NO_EVENTS == 0 {
            if let Some(w) = self.owner(n) {
                if let Some(conn) = self.widgets[w].connections.get(&ev) {
                    if self.widgets[w].enabled || !conn.only_when_enabled {
                        self.events.push(UiEvent::Signal { widget: w, event: ev });
                    }
                }
            }
        }
    }

    /// `0x00652c10 Engine::injectMouseMove(x, y)`: hover change (leave, enter) then
    /// slot 21 and MOUSE_MOVE on the node under the cursor (or the captured node), then
    /// the slot 38 broadcast (a `ret` for every class ported).
    pub fn inject_mouse_move(&mut self, x: f32, y: f32, metrics: Option<&dyn TextMetrics>) {
        self.prev_cursor = self.cursor;
        self.cursor = Vec2::new(x, y);
        let target = self.current_target();
        let old_w = self.hovered.and_then(|h| self.owner(h));
        let new_w = target.and_then(|t| self.owner(t));
        if old_w != new_w {
            if let Some(o) = old_w {
                if self.widgets[o].enabled {
                    self.on_mouse_leave(o);
                }
            }
            if let Some(h) = self.hovered {
                self.fire_mouse(h, event::LEAVE);
            }
            if let Some(nw) = new_w {
                if self.widgets[nw].enabled {
                    self.on_mouse_enter(nw);
                }
            }
            match target {
                None => {
                    self.hovered = None;
                    return;
                }
                Some(t) => self.fire_mouse(t, event::ENTER),
            }
        }
        if let Some(t) = target {
            if let Some(nw) = new_w {
                if self.widgets[nw].enabled {
                    self.on_mouse_move(nw, metrics);
                }
            }
            self.fire_mouse(t, event::MOUSE_MOVE);
        }
        self.hovered = target;
    }

    /// `0x006527f0 Engine::injectLeftDown`.
    pub fn inject_left_down(&mut self, metrics: Option<&dyn TextMetrics>) {
        self.left_down = true;
        let target = self.current_target();
        self.drag_widget = None;
        if let Some(t) = target {
            if let Some(w) = self.owner(t) {
                if self.widgets[w].flags & flags::DRAGGABLE != 0 && self.widgets[w].enabled {
                    self.drag_widget = Some(w);
                    // Slot 28 broadcast: `ret` in every class ported.
                }
            }
        }
        if let Some(p) = self.popup {
            let pn = self.widgets[p].node;
            if !self.is_ancestor_or_self(pn, target) {
                self.set_popup(None, None);
            }
        }
        let focus_node_widget = target.and_then(|t| self.nodes[t].widget);
        if target.is_none() || focus_node_widget != self.focused {
            if let Some(f) = self.focused {
                self.widgets[f].dirty = true;
                let n = self.widgets[f].node;
                self.fire_mouse(n, event::FOCUS_LOST);
            }
            self.focused = None;
        }
        if let Some(t) = target {
            if let Some(w) = self.owner(t) {
                if self.widgets[w].enabled {
                    self.on_mouse_press(w, metrics);
                }
            }
            self.fire_mouse(t, event::LEFT_PRESS);
        }
        // Slot 29 broadcast: `ret` in every class ported.
    }

    /// `0x00652940 Engine::injectLeftUp`.
    pub fn inject_left_up(&mut self) {
        self.left_down = false;
        let target = self.current_target();
        if self.popup.is_some() && self.drag_widget.is_some() {
            self.set_popup(None, None);
        }
        if let Some(t) = target {
            if let Some(w) = self.owner(t) {
                if self.widgets[w].enabled {
                    self.on_mouse_release(w);
                }
            }
            self.fire_mouse(t, event::LEFT_RELEASE);
        }
        if let Some(t) = self.current_target() {
            let under = self.root.and_then(|r| self.hit_test(r, self.cursor, 0));
            let under_w = under.and_then(|u| self.owner(u));
            let tw = self.owner(t);
            if tw != under_w {
                if let Some(w) = tw {
                    if self.widgets[w].enabled {
                        self.on_mouse_leave(w);
                    }
                }
                self.fire_mouse(t, event::LEAVE);
            }
        }
        // Slot 30 broadcast.
        self.broadcast(&mut |g, w| g.on_global_left_release(w));
    }

    /// `0x00652a70 Engine::injectRightDown`.
    pub fn inject_right_down(&mut self) {
        self.right_down = true;
        let target = self.current_target();
        let fw = target.and_then(|t| self.nodes[t].widget);
        if target.is_none() || fw != self.focused {
            if let Some(f) = self.focused {
                self.widgets[f].dirty = true;
                let n = self.widgets[f].node;
                self.fire_mouse(n, event::FOCUS_LOST);
            }
            self.focused = None;
        }
        if let Some(t) = target {
            // Slot 14: `ret` in every class ported.
            self.fire_mouse(t, event::RIGHT_PRESS);
        }
    }

    /// `0x00652b60 Engine::injectRightUp`.
    pub fn inject_right_up(&mut self) {
        self.right_down = false;
        if let Some(t) = self.current_target() {
            // Slot 15: `ret` in every class ported.
            self.fire_mouse(t, event::RIGHT_RELEASE);
        }
    }

    /// `0x00652730 Engine::injectKeyDown`: records the key and forwards it to the
    /// focused, enabled widget's slot 24.
    pub fn inject_key_down(&mut self, key: u16) {
        self.last_key = key;
        if let Some(f) = self.focused {
            if self.widgets[f].enabled {
                self.on_key_down(f, key);
            }
        }
    }

    /// `0x00652790 Engine::injectKeyUp`.
    pub fn inject_key_up(&mut self, key: u16) {
        self.last_key = key;
        if let Some(f) = self.focused {
            if self.widgets[f].enabled {
                self.on_key_up(f, key);
            }
        }
    }

    /// `0x00652710 Engine::injectChar`.
    pub fn inject_char(&mut self, c: u16) {
        if let Some(f) = self.focused {
            if self.widgets[f].enabled {
                self.on_char(f, c);
            }
        }
    }

    /// The per-frame slot 27 `tick` over every widget (the engine traversal that calls
    /// it was not located; assumed to be the same visitor as [`Gui::broadcast`]).
    pub fn tick_all(&mut self, frame_ms: i32) {
        self.frame_ms = frame_ms;
        self.broadcast(&mut |g, w| g.tick(w));
    }
}

/// `InventoryWidget::layout` grid size: `cols = (int)((width − 10) / (cellW + 5))`,
/// `rows = (int)((height − 40) / (cellH + 5))` (truncation toward zero).
pub fn inventory_grid(width: f32, height: f32, cell: IVec2) -> (i32, i32) {
    let cols = ((width - 10.0) / (cell.x + 5) as f32) as i32;
    let rows = ((height - 40.0) / (cell.y + 5) as f32) as i32;
    (cols, rows)
}

/// Position of inventory cell (col, row): `((cellW + 5)·col + 10, (cellH + 5)·row + 40)`.
pub fn inventory_cell_pos(cell: IVec2, col: i32, row: i32) -> Vec2 {
    Vec2::new(((cell.x + 5) * col + 10) as f32, ((cell.y + 5) * row + 40) as f32)
}

// ---------------------------------------------------------------------------------------
// GameController::onResize 0x00482a40
// ---------------------------------------------------------------------------------------

/// The GameController members `onResize` reads, by their offset in `cube::GameController`.
/// Names follow what the GameController ctor 0x00459c40 builds on each member (node names
/// for lookups, captions for cloned buttons, the `cube::*Widget` a panel holds); the
/// client's `ui::members::GcMembers` carries the same names.
#[derive(Clone, Debug, Default)]
pub struct GameControllerWidgets {
    /// +0x80093c: node of the dialog `SpeechWidget` +0x800938 (400x400; y fixed at 300).
    pub speech_node: Option<NodeId>,
    /// +0x800938: the dialog `SpeechWidget`; its width centres +0x80093c.
    pub speech: Option<WidgetId>,
    /// +0x800868 node "questtag".
    pub questtag: Option<NodeId>,
    /// +0x80086c node "smallquesttag".
    pub smallquesttag: Option<NodeId>,
    /// +0x800878 node "cubeworld" (start.plx logo).
    pub cubeworld: Option<NodeId>,
    /// +0x800918: node of the `StartMenuWidget` +0x80091c (200x100, in the start root).
    pub start_menu_node: Option<NodeId>,
    /// +0x80087c node "picroma".
    pub picroma: Option<NodeId>,
    /// +0x800ad0 node "combopoints".
    pub combopoints: Option<NodeId>,
    /// +0x800898 node "wait" ("Please wait...").
    pub wait: Option<NodeId>,
    /// +0x8009f4 node "nameError".
    pub name_error: Option<NodeId>,
    /// +0x8009f8 node "connectionError".
    pub connection_error: Option<NodeId>,
    /// +0x800974 node "charactername" (edit).
    pub character_name: Option<NodeId>,
    /// +0x800970 cloned button "Create Character".
    pub create_character: Option<NodeId>,
    /// +0x80096c: the character style panel (`blackwidget` clone, 300x330, in the creation
    /// screen) holding the `CharacterStyleWidget` +0x800968.
    pub char_style_panel: Option<NodeId>,
    /// +0x800a00: the options panel (450x440) holding the `OptionsWidget` +0x8009fc.
    pub options_panel: Option<NodeId>,
    /// +0x800910: the system menu panel (150x120, key O) holding the `SystemWidget`
    /// +0x800914.
    pub system_panel: Option<NodeId>,
    /// +0x80099c cloned button "Back".
    pub back: Option<NodeId>,
    /// +0x8009a0 cloned button "Multiplayer Worlds...".
    pub multiplayer_worlds: Option<NodeId>,
    /// +0x8009ac node "serverName" (edit).
    pub server_name: Option<NodeId>,
    /// +0x8009a4 cloned button "Connect to server".
    pub connect_to_server: Option<NodeId>,
    /// +0x8009a8 cloned button "Connect".
    pub connect: Option<NodeId>,
    /// +0x8009bc node "worldName" (edit).
    pub world_name: Option<NodeId>,
    /// +0x8009b8 node "worldSeed" (edit).
    pub world_seed: Option<NodeId>,
    /// +0x8009b4 cloned button "Create World".
    pub create_world: Option<NodeId>,
    /// +0x800978..+0x80097c: the character preview nodes (filled by 0x0049d650), centred
    /// horizontally.
    pub character_previews: Vec<NodeId>,
    /// +0x8007fc..+0x800800: the six quick-bar slots (M1 M2 1 2 3 4) in a 24-px-spaced row
    /// at H−50.
    pub quick_slots: Vec<NodeId>,
    /// +0x800838..+0x80083c: the six menu buttons (F1 help, X skills, C crafting,
    /// B inventory, M map, O system) in a 30-px-spaced row at H−140.
    pub menu_buttons: Vec<NodeId>,
    /// +0x8008bc: the inventory panel (400x285, keys B/I) holding the bag
    /// `InventoryWidget` +0x800954 (type 0); the other panels are placed around it.
    pub inventory_panel: Option<NodeId>,
    /// +0x800758 node "landname".
    pub landname: Option<NodeId>,
    /// +0x800a14: the `ChatWidget` (400x200, on node +0x800ad8).
    pub chat: Option<WidgetId>,
    /// +0x8008dc: the weapon customization panel (400x624) holding the `VoxelWidget`
    /// +0x8008f4.
    pub voxel_panel: Option<NodeId>,
    /// +0x800944: node of the `ObjectiveWidget` +0x800948 (fixed at (20, 170)).
    pub objective_node: Option<NodeId>,
    /// +0x80094c: node of the `StatisticsWidget` +0x800950 (fixed at (20, 470)).
    pub statistics_node: Option<NodeId>,
    /// +0x800a34..+0x800a38: the 12 equipment boxes, in a two-column grid.
    pub equipment_boxes: Vec<NodeId>,
    /// +0x800960: the `CharacterWidget` (character sheet); its parent widget, the 260x200
    /// panel, is placed at (20, 340).
    pub character: Option<WidgetId>,
    /// +0x8008c0: the crafting panel (350x285, key C) holding the recipe
    /// `InventoryWidget` +0x800958 (type 2).
    pub crafting_panel: Option<NodeId>,
    /// +0x800ad4: the craft preview panel (350x330) holding the `BlueprintPreviewWidget`
    /// +0x800964.
    pub craft_preview_panel: Option<NodeId>,
    /// +0x800908: the skills panel (300x460, key X) holding the `SkillWidget` +0x80090c.
    pub skills_panel: Option<NodeId>,
    /// +0x8008c4: the shop panel (350x568) holding the shop `InventoryWidget` +0x80095c
    /// (type 3).
    pub shop_panel: Option<NodeId>,
    /// +0x8008f8: the identification panel (350x285) holding the `EnchantWidget` +0x8008fc.
    pub enchant_panel: Option<NodeId>,
    /// +0x800900: the adaption panel (350x400) holding the `AdaptionWidget` +0x800904.
    pub adaption_panel: Option<NodeId>,
    /// +0x80089c: the help overlay (`help.plx`, hidden; F1 and menu button 0 toggle it),
    /// at x = W/2 − 430, y = 50.
    pub help: Option<NodeId>,
}

impl Gui {
    fn set_translation(&mut self, n: NodeId, t: Vec2) {
        // Writing the translation key then Transformation slot 1 `evaluate(1)`.
        self.nodes[n].translation = t;
    }

    fn set_translation_pivot(&mut self, n: NodeId, x: f32, y: f32) {
        let p = self.nodes[n].pivot;
        self.set_translation(n, Vec2::new(x - p.x, y - p.y));
    }

    fn nw(&self, n: Option<NodeId>) -> Option<WidgetId> {
        n.and_then(|n| self.nodes[n].widget)
    }
}

/// `(int)` then `/ 2` as MSVC emits it (`cdq; sub eax, edx; sar eax, 1`: truncation).
fn half(v: i32) -> i32 {
    v / 2
}

/// The unsigned-int-to-float conversion MSVC emits for these expressions
/// (`cvtdq2pd` + `addsd [0x745f30 + sign*8]` then `cvtpd2ps`): the int is reinterpreted
/// as unsigned.
fn u32_to_f32(v: i32) -> f32 {
    (v as u32) as f64 as f32
}

/// `cube::GameController::onResize` 0x00482a40 (slot 4). `w`/`h` are the resolution
/// (GC+0x11c/+0x120, equal to the slot's arguments); `viewport` is the engine's
/// (`Engine+0x10c/+0x110`, read for +0x8008dc). Ends with the engine mouse move to the
/// screen centre (0x00652c10).
pub fn game_controller_on_resize(gui: &mut Gui, gc: &GameControllerWidgets, w: i32, h: i32) {
    let wf = w as f32;
    let hf = h as f32;
    // 0x00482a54: +0x80093c: x = W/2 − width(+0x800938)·0.5, y = 300 (no pivot).
    if let Some(n) = gc.speech_node {
        let half_w = half(w);
        let bw = gc.speech.map_or(0.0, |b| gui.width(b));
        gui.set_translation(n, Vec2::new(half_w as f32 - bw * 0.5, 300.0));
    }
    // 0x00482ac3: "questtag" at (W/2, H−300) minus pivot.
    if let Some(n) = gc.questtag {
        gui.set_translation_pivot(n, half(w) as f32, (h - 300) as f32);
    }
    // 0x00482b2f: "smallquesttag" at (W−200, 400) minus pivot.
    if let Some(n) = gc.smallquesttag {
        gui.set_translation_pivot(n, (w - 200) as f32, 400.0);
    }
    // 0x00482b91: "cubeworld" at (W/2, H/2) minus pivot.
    if let Some(n) = gc.cubeworld {
        gui.set_translation_pivot(n, half(w) as f32, half(h) as f32);
    }
    // 0x00482bfd: widget of +0x800918 at ((W − width)·0.5, H/2 + 50).
    if let Some(x) = gui.nw(gc.start_menu_node) {
        let y = (half(h) + 50) as f32;
        let px = (wf - gui.width(x)) * 0.5;
        gui.set_position_xy(x, px, y, true);
    }
    // 0x00482c5c: "picroma" at (W−20, H−20) minus pivot.
    if let Some(n) = gc.picroma {
        gui.set_translation_pivot(n, (w - 20) as f32, (h - 20) as f32);
    }
    // 0x00482cc4: "combopoints" at (W/2, H−190) minus pivot (not null-checked).
    if let Some(n) = gc.combopoints {
        gui.set_translation_pivot(n, half(w) as f32, (h - 190) as f32);
    }
    // 0x00482d2c: "wait" at (W/2, H/2).
    if let Some(n) = gc.wait {
        gui.set_translation(n, Vec2::new(half(w) as f32, half(h) as f32));
    }
    // 0x00482d7d: "nameError" at (W/2, H−200).
    if let Some(n) = gc.name_error {
        gui.set_translation(n, Vec2::new(half(w) as f32, (h - 200) as f32));
    }
    // 0x00482dce: "connectionError" at (W/2, H/2 + 100).
    if let Some(n) = gc.connection_error {
        gui.set_translation(n, Vec2::new(half(w) as f32, (half(h) + 100) as f32));
    }
    // 0x00482e22: "charactername" at (W/2, H−50).
    if let Some(n) = gc.character_name {
        gui.set_translation(n, Vec2::new(half(w) as f32, (h - 50) as f32));
    }
    // 0x00482e71: "Create Character" bottom-right.
    if let Some(b) = gui.nw(gc.create_character) {
        let y = (h - 20) as f32 - gui.height(b);
        let x = (w - 20) as f32 - gui.width(b);
        gui.set_position_xy(b, x, y, true);
    }
    // 0x00482ed0: +0x80096c at (20, (H−20) − height).
    if let Some(b) = gui.nw(gc.char_style_panel) {
        let y = (h - 20) as f32 - gui.height(b);
        gui.set_position_xy(b, 20.0, y, true);
    }
    // 0x00482f14 / 0x00482f8a: +0x800a00 and +0x800910 centred.
    for n in [gc.options_panel, gc.system_panel] {
        if let Some(b) = gui.nw(n) {
            let y = (hf - gui.height(b)) * 0.5;
            let x = (wf - gui.width(b)) * 0.5;
            gui.set_position_xy(b, x, y, true);
        }
    }
    // 0x00483000: "Back" at (20, 20).
    if let Some(b) = gui.nw(gc.back) {
        gui.set_position_xy(b, 20.0, 20.0, true);
    }
    // 0x00483022: "Multiplayer Worlds..." at ((W − width) − 20, 20).
    if let Some(b) = gui.nw(gc.multiplayer_worlds) {
        let x = (wf - gui.width(b)) - 20.0;
        gui.set_position_xy(b, x, 20.0, true);
    }
    // 0x0048306e: "serverName" at (W/2, H/2 − 100).
    if let Some(n) = gc.server_name {
        gui.set_translation(n, Vec2::new(half(w) as f32, (half(h) - 100) as f32));
    }
    // 0x004830c2: "Connect to server" at ((W − width) − 20, (H − height) − 20).
    if let Some(b) = gui.nw(gc.connect_to_server) {
        let y = (hf - gui.height(b)) - 20.0;
        let x = (wf - gui.width(b)) - 20.0;
        gui.set_position_xy(b, x, y, true);
    }
    // 0x00483138: "Connect" centred.
    if let Some(b) = gui.nw(gc.connect) {
        let y = (hf - gui.height(b)) * 0.5;
        let x = (wf - gui.width(b)) * 0.5;
        gui.set_position_xy(b, x, y, true);
    }
    // 0x004831ae / 0x004831fd: "worldName" at (W/2, H−50), "worldSeed" at (W/2, H−100).
    if let Some(n) = gc.world_name {
        gui.set_translation(n, Vec2::new(half(w) as f32, (h - 50) as f32));
    }
    if let Some(n) = gc.world_seed {
        gui.set_translation(n, Vec2::new(half(w) as f32, (h - 100) as f32));
    }
    // 0x0048324c: "Create World" bottom-right.
    if let Some(b) = gui.nw(gc.create_world) {
        let y = (h - 20) as f32 - gui.height(b);
        let x = (w - 20) as f32 - gui.width(b);
        gui.set_position_xy(b, x, y, true);
    }
    // 0x004832ae: +0x800978 vector: centred horizontally, y kept.
    for &n in &gc.character_previews {
        if let Some(b) = gui.nw(Some(n)) {
            let y = gui.get_position(b).y;
            let x = half(w) as f32 - gui.width(b) * 0.5;
            gui.set_position_xy(b, x, y, true);
        }
    }
    // 0x0048331f: +0x8007fc row: x = W/2 + (2i − n)·24 + 6 (as unsigned), y = H − 50.
    let cnt = gc.quick_slots.len() as i32;
    for (i, &n) in gc.quick_slots.iter().enumerate() {
        let x = half(w) + ((2 * i as i32) - cnt) * 24 + 6;
        gui.set_translation_pivot(n, u32_to_f32(x), (h - 50) as f32);
    }
    // 0x00483422: +0x800838 row: x = W/2 + ((2i − n) + 1)·30 (as unsigned), y = H − 140.
    let cnt = gc.menu_buttons.len() as i32;
    for (i, &n) in gc.menu_buttons.iter().enumerate() {
        let x = half(w) + (((2 * i as i32) - cnt) + 1) * 30;
        gui.set_translation_pivot(n, u32_to_f32(x), (h - 140) as f32);
    }
    // 0x00483531: +0x8008bc at ((W−20) − width, (H − height) − 220), NOT silent.
    if let Some(b) = gui.nw(gc.inventory_panel) {
        let y = (hf - gui.height(b)) - 220.0;
        let x = (w - 20) as f32 - gui.width(b);
        gui.set_position_xy(b, x, y, false);
    }
    // 0x0048359e: "landname" at (W/2, 230) minus pivot.
    if let Some(n) = gc.landname {
        gui.set_translation_pivot(n, half(w) as f32, 230.0);
    }
    // 0x00483600: widget +0x800a14 at (20, H − 220).
    if let Some(b) = gc.chat {
        gui.set_position_xy(b, 20.0, (h - 220) as f32, true);
    }
    // 0x0048362f: +0x8008dc at (vpW/2 − 340, vpH/2 − 250) minus pivot.
    if let Some(n) = gc.voxel_panel {
        let vp = gui.viewport;
        gui.set_translation_pivot(n, (half(vp.x) - 340) as f32, (half(vp.y) - 250) as f32);
    }
    // 0x004836b5 / 0x004836ea: fixed translations.
    if let Some(n) = gc.objective_node {
        gui.set_translation(n, Vec2::new(20.0, 170.0));
    }
    if let Some(n) = gc.statistics_node {
        gui.set_translation(n, Vec2::new(20.0, 470.0));
    }
    // 0x0048371f: +0x800a34 two-column grid: x = W/2 − 200 + (i % 2)·400,
    // y = H/2 + (i / 2)·56 + 10, minus pivot.
    for (i, &n) in gc.equipment_boxes.iter().enumerate() {
        let i = i as i32;
        let x = half(w) - 200 + (i % 2) * 400;
        let y = half(h) + (i / 2) * 56 + 10;
        gui.set_translation_pivot(n, x as f32, y as f32);
    }
    // 0x00483830: the parent widget of +0x800960 at (20, 340).
    if let Some(b) = gc.character.and_then(|b| gui.parent_widget(b)) {
        gui.set_position_xy(b, 20.0, 340.0, true);
    }
    // 0x00483856 onwards: panels arranged around +0x8008bc, all through setPosition.
    let main = gui.nw(gc.inventory_panel);
    if let Some(m) = main {
        let x = half(w) as f32 - gui.width(m) * 0.5;
        let y = half(h) as f32 - gui.height(m);
        gui.set_position(m, Vec2::new(x, y), true);
    }
    // Left of main: x = ((W/2 − wMain·0.5) − wThis) − 10, y = H/2 − hMain.
    let left_of_main = |gui: &mut Gui, n: Option<NodeId>| {
        if let (Some(b), Some(m)) = (gui.nw(n), main) {
            let x = ((half(w) as f32 - gui.width(m) * 0.5) - gui.width(b)) - 10.0;
            let y = half(h) as f32 - gui.height(m);
            gui.set_position(b, Vec2::new(x, y), true);
        }
    };
    left_of_main(gui, gc.crafting_panel);
    // +0x800ad4: left of main, below +0x8008c0: y = hC0 + ((H/2 − hMain) + 10).
    if let (Some(b), Some(m), Some(c0)) = (gui.nw(gc.craft_preview_panel), main, gui.nw(gc.crafting_panel)) {
        let x = ((half(w) as f32 - gui.width(m) * 0.5) - gui.width(b)) - 10.0;
        let y = gui.height(c0) + ((half(h) as f32 - gui.height(m)) + 10.0);
        gui.set_position(b, Vec2::new(x, y), true);
    }
    left_of_main(gui, gc.skills_panel);
    // Right of main: x = (wMain·0.5 + W/2) + 10, y = H/2 − hMain.
    for n in [gc.shop_panel, gc.enchant_panel, gc.adaption_panel] {
        if let (Some(b), Some(m)) = (gui.nw(n), main) {
            let x = (gui.width(m) * 0.5 + half(w) as f32) + 10.0;
            let y = half(h) as f32 - gui.height(m);
            gui.set_position(b, Vec2::new(x, y), true);
        }
    }
    left_of_main(gui, gc.voxel_panel);
    // 0x00483dda: +0x80089c at (W/2 − 430, 50) (no pivot, not null-checked).
    if let Some(n) = gc.help {
        gui.set_translation(n, Vec2::new((half(w) - 430) as f32, 50.0));
    }
    // 0x00483e20: centre the cursor.
    gui.inject_mouse_move(half(w) as f32, half(h) as f32, None);
}

// ---------------------------------------------------------------------------------------
// Vtable maps (for reviewers)
// ---------------------------------------------------------------------------------------

/// `plasma::Widget` vtable 0x0071e724 slot → Rust method.
pub const WIDGET_SLOTS: &[(u32, u32, &str)] = &[
    (0, 0x00428a30, "drop (scalar deleting dtor)"),
    (1, 0x004114b0, "Gui::frame_update (ret)"),
    (2, 0x00687cf0, "size getter returning (0,0) (not needed)"),
    (3, 0x00687cf0, "Gui::content_size"),
    (4, 0x00411330, "Gui::slot4"),
    (5, 0x00687d10, "Gui::contains_point"),
    (6, 0x004114b0, "ret"),
    (7, 0x0062a8b0, "Gui::update"),
    (8, 0x0062b350, "Gui::on_viewport_resized"),
    (9, 0x0062af10, "Gui::on_parent_resized"),
    (10, 0x004114b0, "Gui::layout"),
    (11, 0x0062a9e0, "Gui::on_mouse_press"),
    (12, 0x0062abe0, "Gui::on_mouse_release"),
    (13, 0x004114b0, "Gui::on_mouse_double_click"),
    (14, 0x004114b0, "right press (ret; Gui::inject_right_down)"),
    (15, 0x004114b0, "right release (ret; Gui::inject_right_up)"),
    (21, 0x0062ac20, "Gui::on_mouse_move"),
    (22, 0x004114b0, "Gui::on_mouse_enter"),
    (23, 0x004114b0, "Gui::on_mouse_leave"),
    (24, 0x00675660, "Gui::on_key_down"),
    (25, 0x00675660, "Gui::on_key_up"),
    (26, 0x00675660, "Gui::on_char"),
    (27, 0x004114b0, "Gui::tick"),
    (28, 0x004114b0, "drag-start broadcast (ret)"),
    (29, 0x004114b0, "left-down broadcast (ret)"),
    (30, 0x004114b0, "Gui::on_global_left_release"),
    (32, 0x004114b0, "right-down broadcast (ret)"),
    (33, 0x004114b0, "right-up broadcast (ret)"),
    (35, 0x0062a690, "Gui::on_drag_begin"),
    (36, 0x0062a780, "Gui::on_drag_end"),
    (38, 0x004114b0, "mouse-move broadcast (ret)"),
    (39, 0x0062a7b0, "Gui::on_mouse_wheel"),
    (40, 0x00627dc0, "Gui::clone_widget"),
    (41, 0x00628f40, "Gui::set_enabled"),
    (42, 0x004114b0, "Gui::on_scrolled"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn widget_src(size: Vec2) -> WidgetSource {
        WidgetSource { inner_bind_size: size, bind_size: size, frame_size: size, ..Default::default() }
    }

    /// Root node + a root widget of `vp` size, and a child widget node at `pos`.
    fn setup(vp: IVec2, pos: Vec2, size: Vec2, src: WidgetSource) -> (Gui, WidgetId, WidgetId) {
        let mut g = Gui::new();
        g.viewport = vp;
        let root = g.add_plain_node(None, "root");
        let rw = g.add_widget(root, &widget_src(vp.as_vec2()));
        let cn = g.add_node(Some(root), NodeSource { name: "c".into(), translation: pos, visible: true, ..Default::default() });
        let mut s = src;
        s.inner_bind_size = size;
        s.bind_size = size;
        s.frame_size = size;
        let cw = g.add_widget(cn, &s);
        g.update(rw);
        g.update(cw);
        (g, rw, cw)
    }

    #[test]
    fn effective_size_clamps_to_fixed_part() {
        // bind 100, inner 60 → fixed part 40; frame 10 is below it.
        assert_eq!(effective_axis(60.0, 10.0, 40.0, 100.0), 40.0);
        assert_eq!(effective_axis(60.0, 150.0, 40.0, 100.0), 150.0);
        assert_eq!(stretch_axis(60.0, 150.0, 40.0, 100.0), 110.0);
    }

    #[test]
    fn anchor_right_bottom_moves_by_delta() {
        let s = WidgetSource { horizontal_alignment: 1, vertical_alignment: 1, ..Default::default() };
        let (mut g, rw, cw) = setup(IVec2::new(800, 600), Vec2::new(700.0, 500.0), Vec2::new(50.0, 40.0), s);
        // The root widget has no parent node, so its slot 9 returns; drive the child.
        g.viewport = IVec2::new(1000, 700);
        g.on_viewport_resized(rw);
        g.on_parent_resized(cw, Vec2::ZERO, Vec2::new(200.0, 100.0));
        assert_eq!(g.get_position(cw), Vec2::new(900.0, 600.0));
    }

    #[test]
    fn anchor_centre_moves_by_half_delta() {
        let s = WidgetSource { horizontal_alignment: 2, vertical_alignment: 2, ..Default::default() };
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(375.0, 280.0), Vec2::new(50.0, 40.0), s);
        g.on_parent_resized(cw, Vec2::ZERO, Vec2::new(200.0, 100.0));
        assert_eq!(g.get_position(cw), Vec2::new(475.0, 330.0));
    }

    #[test]
    fn anchor_left_top_stays() {
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(10.0, 20.0), Vec2::new(50.0, 40.0), WidgetSource::default());
        g.on_parent_resized(cw, Vec2::ZERO, Vec2::new(200.0, 100.0));
        assert_eq!(g.get_position(cw), Vec2::new(10.0, 20.0));
    }

    #[test]
    fn stretch_grows_size_and_suppresses_right_anchor() {
        let s = WidgetSource {
            horizontal_alignment: 1,
            flags: flags::STRETCH_X | flags::STRETCH_Y,
            ..Default::default()
        };
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(10.0, 10.0), Vec2::new(100.0, 50.0), s);
        g.on_parent_resized(cw, Vec2::ZERO, Vec2::new(200.0, 100.0));
        assert_eq!(g.get_position(cw), Vec2::new(10.0, 10.0));
        assert_eq!(g.get_size(cw), Vec2::new(300.0, 150.0));
        assert!(g.widgets[cw].dirty);
    }

    #[test]
    fn viewport_resize_reaches_children_through_stretch() {
        // A stretching root widget under the root node propagates to an anchored child.
        let mut g = Gui::new();
        g.viewport = IVec2::new(800, 600);
        let root = g.add_plain_node(None, "root");
        let pn = g.add_plain_node(Some(root), "panel");
        let size = Vec2::new(800.0, 600.0);
        let pw = g.add_widget(
            pn,
            &WidgetSource {
                inner_bind_size: size,
                bind_size: size,
                frame_size: size,
                flags: flags::STRETCH_X | flags::STRETCH_Y,
                ..Default::default()
            },
        );
        let cn = g.add_node(Some(pn), NodeSource { name: "b".into(), translation: Vec2::new(700.0, 550.0), visible: true, ..Default::default() });
        let csize = Vec2::new(80.0, 30.0);
        let cw = g.add_widget(
            cn,
            &WidgetSource {
                inner_bind_size: csize,
                bind_size: csize,
                frame_size: csize,
                horizontal_alignment: 1,
                vertical_alignment: 1,
                ..Default::default()
            },
        );
        g.update(pw);
        g.update(cw);
        g.viewport = IVec2::new(1024, 768);
        g.on_viewport_resized(pw);
        assert_eq!(g.get_size(pw), Vec2::new(1024.0, 768.0));
        assert_eq!(g.get_position(cw), Vec2::new(924.0, 718.0));
        assert_eq!(g.widgets[pw].last_viewport, IVec2::new(1024, 768));
    }

    #[test]
    fn edge_resize_right_edge() {
        let size = Vec2::new(100.0, 100.0);
        // Inner rect inset by 10 on each side so the border is grabbable.
        let s = WidgetSource {
            inner_bind_pos: Vec2::new(10.0, 10.0),
            flags: flags::RESIZABLE,
            ..Default::default()
        };
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(100.0, 100.0), size, s);
        // setup overwrote inner size to `size`; make it 80 so the border is 10 px.
        g.widgets[cw].inner_bind_size = Vec2::new(80.0, 80.0);
        g.update(cw);
        g.cursor = Vec2::new(195.0, 150.0);
        g.on_mouse_press(cw, None);
        assert_eq!(g.widgets[cw].resize_edge, [1, 0]);
        assert_eq!(g.captured, Some(g.widgets[cw].node));
        g.left_down = true;
        g.prev_cursor = g.cursor;
        g.cursor = Vec2::new(215.0, 150.0);
        g.on_mouse_move(cw, None);
        assert_eq!(g.get_size(cw), Vec2::new(120.0, 100.0));
        assert_eq!(g.get_position(cw), Vec2::new(100.0, 100.0));
    }

    #[test]
    fn edge_resize_left_edge_moves_origin() {
        let s = WidgetSource { inner_bind_pos: Vec2::new(10.0, 10.0), flags: flags::RESIZABLE, ..Default::default() };
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(100.0, 100.0), Vec2::new(100.0, 100.0), s);
        g.widgets[cw].inner_bind_size = Vec2::new(80.0, 80.0);
        g.update(cw);
        g.cursor = Vec2::new(103.0, 150.0);
        g.on_mouse_press(cw, None);
        assert_eq!(g.widgets[cw].resize_edge, [-1, 0]);
        g.left_down = true;
        g.prev_cursor = g.cursor;
        g.cursor = Vec2::new(93.0, 150.0);
        g.on_mouse_move(cw, None);
        assert_eq!(g.get_position(cw), Vec2::new(90.0, 100.0));
        assert_eq!(g.get_size(cw), Vec2::new(110.0, 100.0));
    }

    #[test]
    fn drag_move_follows_cursor_on_movable_axes() {
        let s = WidgetSource { flags: flags::MOVABLE_X, ..Default::default() };
        let (mut g, _rw, cw) = setup(IVec2::new(800, 600), Vec2::new(0.0, 0.0), Vec2::new(2000.0, 50.0), s);
        let node = g.widgets[cw].node;
        g.widgets[cw].drag_start_cursor = Vec2::new(100.0, 100.0);
        g.widgets[cw].drag_start_translation = Vec2::ZERO;
        g.drag_move_active = true;
        g.cursor = Vec2::new(60.0, 130.0);
        g.on_mouse_move(cw, None);
        // x follows (−40), y is locked; then clamped inside the 800-wide parent.
        assert_eq!(g.nodes[node].translation, Vec2::new(-40.0, 0.0));
        g.cursor = Vec2::new(-2000.0, 130.0);
        g.on_mouse_move(cw, None);
        assert_eq!(g.nodes[node].translation.x, 800.0 - 2000.0);
    }

    #[test]
    fn button_auto_repeat_timing() {
        let s = WidgetSource { kind: WidgetSourceKind::Button { button_type: 0 }, ..Default::default() };
        let (mut g, _rw, bw) = setup(IVec2::new(800, 600), Vec2::ZERO, Vec2::new(50.0, 20.0), s);
        g.connect(bw, event::REPEAT, Connection::default());
        g.on_mouse_press(bw, None);
        g.events.clear();
        let mut times = Vec::new();
        for ms in (10..=2000).step_by(10) {
            g.frame_ms = 10;
            g.tick(bw);
            let n = g.events.iter().filter(|e| matches!(e, UiEvent::Signal { event: event::REPEAT, .. })).count();
            for _ in 0..n {
                times.push(ms);
            }
            g.events.clear();
        }
        // First repeat once the accumulator passes 150 (650 ms), then 140, 130 ... 50.
        assert_eq!(times[0], 660);
        assert_eq!(times[1], 660 + 140);
        assert_eq!(times[2], 660 + 140 + 130);
        let gaps: Vec<i32> = times.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().rev().take(3).all(|&g| g == 50));
    }

    #[test]
    fn radio_group_keeps_single_checked() {
        let mut g = Gui::new();
        g.viewport = IVec2::new(800, 600);
        let root = g.add_plain_node(None, "root");
        let panel = g.add_widget(root, &WidgetSource::default());
        let mut buttons = Vec::new();
        for i in 0..3 {
            let n = g.add_plain_node(Some(root), &format!("r{i}"));
            buttons.push(g.add_widget(n, &WidgetSource { kind: WidgetSourceKind::Button { button_type: 2 }, ..Default::default() }));
        }
        g.update(panel);
        for &b in &buttons {
            g.update(b);
        }
        g.on_mouse_press(buttons[0], None);
        g.on_mouse_press(buttons[1], None);
        g.on_mouse_press(buttons[1], None);
        let checked: Vec<bool> = buttons.iter().map(|&b| g.button(b).unwrap().checked).collect();
        assert_eq!(checked, vec![false, true, false]);
    }

    #[test]
    fn edit_filters() {
        let s = |t: &str| t.encode_utf16().collect::<Vec<u16>>();
        assert!(EditFilter::Integer.accept(&s("")));
        assert!(EditFilter::Integer.accept(&s("-12")));
        assert!(EditFilter::Integer.accept(&s("-")));
        assert!(!EditFilter::Integer.accept(&s("1-2")));
        assert!(!EditFilter::UnsignedInteger.accept(&s("-1")));
        assert!(EditFilter::Float.accept(&s("-1,5")));
        assert!(!EditFilter::Float.accept(&s("1.2.3")));
        assert!(!EditFilter::Float.accept(&s("1.2,3")));
        assert!(EditFilter::UnsignedFloat.accept(&s("3.25")));
        assert!(!EditFilter::UnsignedFloat.accept(&s("-3.25")));
    }

    fn edit_setup(text: &str) -> (Gui, WidgetId) {
        let mut g = Gui::new();
        g.viewport = IVec2::new(800, 600);
        let root = g.add_plain_node(None, "root");
        let n = g.add_node(Some(root), NodeSource { name: "e".into(), visible: true, text: Some(text.into()), ..Default::default() });
        let w = g.add_widget(n, &WidgetSource { kind: WidgetSourceKind::Edit, ..Default::default() });
        g.update(w);
        (g, w)
    }

    #[test]
    fn edit_typing_and_filter() {
        let (mut g, w) = edit_setup("12");
        g.set_edit_filter(w, Some(EditFilter::Integer));
        g.edit_mut(w).caret = 2;
        g.set_focus(Some(w));
        g.inject_char(b'3' as u16);
        g.inject_char(b'x' as u16);
        assert_eq!(g.edit_text(w), "123");
        g.inject_key_down(vk::HOME);
        g.inject_char(b'-' as u16);
        assert_eq!(g.edit_text(w), "-123");
        g.inject_key_down(vk::BACK);
        assert_eq!(g.edit_text(w), "123");
        // Delete with no selection removes the character after the caret.
        g.inject_key_down(vk::DELETE);
        assert_eq!(g.edit_text(w), "23");
    }

    #[test]
    fn edit_shift_selection_replaced_by_char() {
        let (mut g, w) = edit_setup("hello");
        g.set_focus(Some(w));
        g.edit_mut(w).caret = 5;
        g.keys_down.insert(vk::SHIFT);
        g.inject_key_down(vk::LEFT);
        g.inject_key_down(vk::LEFT);
        g.keys_down.remove(&vk::SHIFT);
        assert_eq!((g.edit(w).caret, g.edit(w).selection), (3, 2));
        g.inject_char(b'p' as u16);
        assert_eq!(g.edit_text(w), "help");
    }

    #[derive(Debug)]
    struct Rect(Vec2, Vec2);
    impl HitShape for Rect {
        fn contains_point(&self, p: Vec2) -> bool {
            p.x >= self.0.x && p.y >= self.0.y && p.x < self.1.x && p.y < self.1.y
        }
        fn bounds(&self) -> (Vec2, Vec2) {
            (self.0, self.1)
        }
    }

    #[test]
    fn hit_test_topmost_child_wins() {
        let mut g = Gui::new();
        let root = g.add_plain_node(None, "root");
        let a = g.add_plain_node(Some(root), "a");
        let b = g.add_plain_node(Some(root), "b");
        g.nodes[a].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::splat(100.0))));
        g.nodes[b].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::splat(50.0))));
        let inner = g.add_node(Some(b), NodeSource { name: "bi".into(), translation: Vec2::splat(10.0), visible: true, ..Default::default() });
        g.nodes[inner].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::splat(10.0))));
        // Overlap: b (later) wins over a; b's child wins over b.
        assert_eq!(g.hit_test(root, Vec2::splat(15.0), 0), Some(inner));
        assert_eq!(g.hit_test(root, Vec2::splat(30.0), 0), Some(b));
        assert_eq!(g.hit_test(root, Vec2::splat(70.0), 0), Some(a));
        g.nodes[b].visible = false;
        assert_eq!(g.hit_test(root, Vec2::splat(15.0), 0), Some(a));
        // NO_EVENTS nodes are skipped unless filter bit 2.
        g.nodes[a].flags = node_flags::NO_EVENTS;
        assert_eq!(g.hit_test(root, Vec2::splat(70.0), 0), None);
        assert_eq!(g.hit_test(root, Vec2::splat(70.0), 2), Some(a));
    }

    /// A shape whose owner widget is deformed (a template resized, like every `blackwidget`
    /// clone of the select screens and every resized button). The cursor must be taken into
    /// the widget's bind space before the 9-slice map is undone (0x0062e180), as the drawer
    /// does the other way round (`0x0068ab70`); undeforming node-local coordinates missed the
    /// shape whenever the widget's frame origin is not the node origin.
    #[test]
    fn undeform_goes_through_bind_space() {
        #[derive(Debug)]
        struct Deformed(Vec2, Vec2);
        impl HitShape for Deformed {
            fn contains_point(&self, p: Vec2) -> bool {
                p.x >= self.0.x && p.y >= self.0.y && p.x < self.1.x && p.y < self.1.y
            }
            fn bounds(&self) -> (Vec2, Vec2) {
                (self.0, self.1)
            }
            fn is_deformed(&self) -> bool {
                true
            }
        }
        let mut g = Gui::new();
        let root = g.add_plain_node(None, "root");
        g.root = Some(root);
        let n = g.add_node(Some(root), NodeSource { name: "panel".into(), translation: Vec2::splat(200.0), visible: true, ..Default::default() });
        // The file's bind matrix takes node-local to widget space: widget (0, 0) is node
        // (-50, -50). A 10x10 template with a 6x6 stretchable middle, stretched to 100x100.
        let mut bm = IDENTITY16;
        bm[12] = 50.0;
        bm[13] = 50.0;
        let src = WidgetSource {
            inner_bind_pos: Vec2::splat(2.0),
            inner_bind_size: Vec2::splat(6.0),
            bind_size: Vec2::splat(10.0),
            frame_size: Vec2::splat(100.0),
            bind_matrix: bm,
            ..Default::default()
        };
        let w = g.add_widget(n, &src);
        g.nodes[n].shape = Some(Box::new(Deformed(Vec2::splat(-50.0), Vec2::splat(-40.0))));
        // Widget space (50, 50), the middle of the stretched panel: node-local (0, 0).
        g.cursor = Vec2::splat(200.0);
        assert!(g.widget_under_cursor(w, n));
        assert_eq!(g.hit_test(root, g.cursor, 0), Some(n));
        // Widget space (99, 99): the bottom-right corner is still inside.
        g.cursor = Vec2::splat(249.0);
        assert!(g.widget_under_cursor(w, n));
        // Outside the stretched frame.
        g.cursor = Vec2::splat(251.0);
        assert!(!g.widget_under_cursor(w, n));
        assert_eq!(g.hit_test(root, g.cursor, 0), None);
    }

    #[test]
    fn clip_restricts_children() {
        let mut g = Gui::new();
        let root = g.add_plain_node(None, "root");
        let a = g.add_node(Some(root), NodeSource { name: "a".into(), visible: true, clip: true, ..Default::default() });
        g.nodes[a].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::splat(20.0))));
        let c = g.add_plain_node(Some(a), "c");
        g.nodes[c].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::splat(100.0))));
        assert_eq!(g.hit_test(root, Vec2::splat(10.0), 0), Some(c));
        assert_eq!(g.hit_test(root, Vec2::splat(50.0), 0), None);
    }

    #[test]
    fn mouse_dispatch_fires_press_and_hover() {
        let mut g = Gui::new();
        g.viewport = IVec2::new(800, 600);
        let root = g.add_plain_node(None, "root");
        let n = g.add_plain_node(Some(root), "btn");
        g.nodes[n].shape = Some(Box::new(Rect(Vec2::ZERO, Vec2::new(100.0, 30.0))));
        let b = g.add_widget(n, &WidgetSource { kind: WidgetSourceKind::Button { button_type: 1 }, ..Default::default() });
        g.update(b);
        for e in [event::ENTER, event::LEFT_PRESS, event::LEFT_RELEASE, event::LEAVE] {
            g.connect(b, e, Connection::default());
        }
        g.events.clear();
        g.inject_mouse_move(10.0, 10.0, None);
        g.inject_left_down(None);
        g.inject_left_up();
        g.inject_mouse_move(500.0, 500.0, None);
        let sigs: Vec<u32> = g
            .events
            .iter()
            .filter_map(|e| match e {
                UiEvent::Signal { event, .. } => Some(*event),
                _ => None,
            })
            .collect();
        assert_eq!(sigs, vec![event::ENTER, event::LEFT_PRESS, event::LEFT_RELEASE, event::LEAVE]);
        assert!(g.button(b).unwrap().checked);
        assert_eq!(g.captured, None);
    }

    #[test]
    fn deform_nine_slice() {
        let d = DeformUniforms {
            bind_pos: Vec2::new(10.0, 10.0),
            bind_size: Vec2::new(80.0, 80.0),
            deformed_pos: Vec2::new(10.0, 10.0),
            deformed_size: Vec2::new(180.0, 80.0),
        };
        assert_eq!(d.deform(Vec2::new(5.0, 5.0)), Vec2::new(5.0, 5.0));
        assert_eq!(d.deform(Vec2::new(50.0, 50.0)), Vec2::new(100.0, 50.0));
        assert_eq!(d.deform(Vec2::new(95.0, 50.0)), Vec2::new(195.0, 50.0));
        for p in [Vec2::new(5.0, 5.0), Vec2::new(50.0, 50.0), Vec2::new(95.0, 95.0)] {
            assert_eq!(d.undeform(d.deform(p)), p);
        }
    }

    #[test]
    fn transformation_rotates_about_pivot() {
        let id = IDENTITY16;
        // 90 degrees about z around the pivot (10, 0), then translated by (5, 5).
        let m = evaluate_transformation(Vec2::new(5.0, 5.0), [0.0, 0.0, 90.0], Vec2::new(10.0, 0.0), &id);
        let a = affine_from_d3d(&m);
        let p = a.transform_point2(Vec2::new(20.0, 0.0));
        // (20,0) - pivot = (10,0) -> rotated (0,10) (row-vector: x' = x·m0 + y·m4) -> + pivot + t.
        assert!((p - Vec2::new(15.0, 15.0)).length() < 1e-4, "{p:?}");
        assert_eq!((m[2], m[6], m[10], m[14]), (0.0, 0.0, 1.0, 0.0));
        // Identity rotation and deformation: pure translation.
        let m = evaluate_transformation(Vec2::new(3.0, 4.0), [0.0; 3], Vec2::ZERO, &id);
        assert_eq!((m[12], m[13]), (3.0, 4.0));
    }

    #[test]
    fn inventory_grid_truncates() {
        assert_eq!(inventory_grid(300.0, 250.0, IVec2::new(40, 40)), (6, 4));
        assert_eq!(inventory_cell_pos(IVec2::new(40, 40), 2, 1), Vec2::new(100.0, 85.0));
    }

    #[test]
    fn on_resize_places_game_widgets() {
        let mut g = Gui::new();
        g.viewport = IVec2::new(1280, 720);
        let root = g.add_plain_node(None, "root");
        let mk = |g: &mut Gui, name: &str, size: Vec2| {
            let n = g.add_plain_node(Some(root), name);
            let w = g.add_widget(n, &WidgetSource { inner_bind_size: size, bind_size: size, frame_size: size, ..Default::default() });
            g.update(w);
            n
        };
        let back = mk(&mut g, "back", Vec2::new(100.0, 30.0));
        let mp = mk(&mut g, "mp", Vec2::new(200.0, 30.0));
        let cw = mk(&mut g, "createworld", Vec2::new(150.0, 40.0));
        let connect = mk(&mut g, "connect", Vec2::new(120.0, 30.0));
        let wait = g.add_plain_node(Some(root), "wait");
        let quest = g.add_node(Some(root), NodeSource { name: "questtag".into(), pivot: Vec2::new(16.0, 8.0), visible: true, ..Default::default() });
        let row: Vec<NodeId> = (0..3).map(|i| g.add_plain_node(Some(root), &format!("s{i}"))).collect();
        let gc = GameControllerWidgets {
            back: Some(back),
            multiplayer_worlds: Some(mp),
            create_world: Some(cw),
            connect: Some(connect),
            wait: Some(wait),
            questtag: Some(quest),
            quick_slots: row.clone(),
            ..Default::default()
        };
        game_controller_on_resize(&mut g, &gc, 1280, 720);
        let pos = |g: &Gui, n: NodeId| g.get_position(g.nodes[n].widget.unwrap());
        assert_eq!(pos(&g, back), Vec2::new(20.0, 20.0));
        assert_eq!(pos(&g, mp), Vec2::new(1060.0, 20.0));
        assert_eq!(pos(&g, cw), Vec2::new(1110.0, 660.0));
        assert_eq!(pos(&g, connect), Vec2::new(580.0, 345.0));
        assert_eq!(g.nodes[wait].translation, Vec2::new(640.0, 360.0));
        assert_eq!(g.nodes[quest].translation, Vec2::new(624.0, 412.0));
        let xs: Vec<f32> = row.iter().map(|&n| g.nodes[n].translation.x).collect();
        assert_eq!(xs, vec![640.0 - 72.0 + 6.0, 640.0 - 24.0 + 6.0, 640.0 + 24.0 + 6.0]);
        assert_eq!(g.cursor, Vec2::new(640.0, 360.0));
    }
}
