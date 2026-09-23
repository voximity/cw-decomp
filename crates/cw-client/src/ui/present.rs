//! The presentation of [`super::FrameOutput`]: what `GameUi::frame` computed for the widgets
//! (texts, fills, positions, visibility) written into the widget tree ([`apply`], after the
//! UI frame in `update`) and the game widgets' slot-1 text calls handed to
//! `cw_ui::render::render_gui` ([`widget_texts`], in `render`).
//!
//! The per-area producers live in their own modules (`present_hud`, `present_panels`); this
//! module only dispatches.

use std::collections::BTreeMap;

use cw_ui::render::WidgetText;
use cw_ui::widget::WidgetId;

use super::{FrameOutput, GameUi, GameView};

/// Writes this frame's widget state into the tree (node visibility, translations, scales,
/// the text of text nodes). Runs after `GameUi::frame` in `Controller::ui_frame`.
pub fn apply(ui: &mut GameUi, game: &GameView, out: &FrameOutput) {
    super::present_hud::apply(ui, game, out);
    super::present_panels::apply(ui, game, out);
}

/// The game widgets' slot-1 text calls of this frame, by widget.
pub fn widget_texts(ui: &GameUi, game: &GameView, out: &FrameOutput, texts: &mut BTreeMap<WidgetId, Vec<WidgetText>>) {
    super::present_hud::widget_texts(ui, game, out, texts);
    super::present_panels::widget_texts(ui, game, out, texts);
}
