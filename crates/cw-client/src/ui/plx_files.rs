//! The `.plx` loading of the GameController ctor 0x00459c40 over cw-ui's scene builder:
//! `Engine::loadFile(node, path, 0x20)` 0x00653770 → `PlxReader::read` 0x00681c70, which is
//! [`cw_formats::plx::parse`] (the "PlasmaXGraphics" seal key among the client's candidate
//! keys) followed by [`cw_ui::loader::load_into`] with the ctor's reader flags
//! ([`cw_ui::loader::reader_flags::GAME_CONTROLLER`], `push 0x20` before each call, e.g.
//! 0x0045c143).
//!
//! The five files, in ctor order: `gui.plx` and `quest-tag.plx` into the GUI root
//! (+0x800884), `help.plx` into the hidden help node (+0x80089c), `start.plx` into the start
//! root (+0x800874), `cursor.plx` into the cursor node (+0x8008a8). The original resolves
//! the names against the working directory (the game folder); [`GamePlxLoader`] takes that
//! folder explicitly. A missing or unreadable file loads nothing, as `loadFile` does when
//! `fopen` fails (the named nodes are then missing, which the ctor does not check).

use std::path::PathBuf;

use cw_ui::loader::{LoadOptions, PlxScene, SharedShape, Texture};
use cw_ui::widget::{Gui, NodeId};

use super::members::PlxLoader;

/// One loaded file: its name, the node it was loaded into and the scene objects (textures,
/// shapes, transformations, displays, animation) the renderer and the animation step use.
#[derive(Debug)]
pub struct LoadedPlx {
    /// `gui.plx`, ...
    pub file: String,
    /// The `loadFile` target node.
    pub target: NodeId,
    /// What the reader built besides the node tree.
    pub scene: PlxScene,
}

/// [`PlxLoader`] reading the shipped files from the game directory.
#[derive(Debug, Default)]
pub struct GamePlxLoader {
    /// The game folder (`CW_GAME_DIR`).
    pub dir: PathBuf,
    /// The files loaded so far, in load order.
    pub loaded: Vec<LoadedPlx>,
    /// Files that could not be read or parsed: `(file, error)`.
    pub errors: Vec<(String, String)>,
}

impl GamePlxLoader {
    /// A loader over `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        GamePlxLoader { dir: dir.into(), loaded: Vec::new(), errors: Vec::new() }
    }

    /// The scene of `file`, once loaded.
    pub fn scene(&self, file: &str) -> Option<&PlxScene> {
        self.loaded.iter().find(|l| l.file == file).map(|l| &l.scene)
    }
}

impl PlxLoader for GamePlxLoader {
    fn game_dir(&self) -> Option<PathBuf> {
        Some(self.dir.clone())
    }

    fn load(&mut self, gui: &mut Gui, file: &str, parent: NodeId) {
        let path = self.dir.join(file);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.errors.push((file.to_string(), e.to_string()));
                return;
            }
        };
        let doc = match cw_formats::plx::parse(&bytes) {
            Ok(d) => d,
            Err(e) => {
                self.errors.push((file.to_string(), e.to_string()));
                return;
            }
        };
        let scene = cw_ui::loader::load_into(gui, Some(parent), &doc, LoadOptions::default());
        self.loaded.push(LoadedPlx { file: file.to_string(), target: parent, scene });
    }

    fn add_texture(&mut self, file: &str, name: &str) -> Option<i32> {
        let db = cw_formats::AssetDb::open(self.dir.join("data3.db")).ok()?;
        let png = db.get(name).ok()?;
        let img = match image::load_from_memory(&png) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.errors.push((name.to_string(), e.to_string()));
                return None;
            }
        };
        let scene = &mut self.loaded.iter_mut().find(|l| l.file == file)?.scene;
        let index = scene.textures.len() as i32;
        scene.textures.push(Texture {
            name: name.to_string(),
            id: -1,
            width: img.width() as i32,
            height: img.height() as i32,
            // The format record (1, 0, 0, 1, 1) of 0x004664e0: A8R8G8B8, point, wrap.
            pixel_format: 1,
            min_filter: 0,
            max_filter: 0,
            horizontal_wrap: 1,
            vertical_wrap: 1,
            pixels: img.into_raw(),
        });
        Some(index)
    }

    fn register_clones(&mut self, gui: &Gui, pairs: &[(NodeId, NodeId)]) {
        for &(src, clone) in pairs {
            let Some(l) = self.loaded.iter_mut().find(|l| l.scene.node_objects.contains_key(&src)) else { continue };
            let sc = &mut l.scene;
            let (ti, di, _) = sc.node_objects[&src];
            // Own copies of the Transformation and Display (0x00636b70 / 0x006368e0), and the
            // clone's own shape (slot 13 made a deep copy) registered for the animation step.
            // A sequence playing on the template does not carry over (assumed).
            let mut t = sc.transformations[ti].clone();
            t.active = false;
            sc.transformations.push(t);
            let mut d = sc.displays[di].clone();
            d.active = false;
            sc.displays.push(d);
            let si = gui.nodes[clone].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<SharedShape>()).map(|s| {
                sc.shapes.push(s.clone());
                sc.shapes.len() - 1
            });
            sc.node_objects.insert(clone, (sc.transformations.len() - 1, sc.displays.len() - 1, si));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{FrameInputs, GameUi, GameView, UiAction, flow};
    use cw_ui::widget::{UiEvent, event};
    use glam::IVec2;

    fn game_dir() -> Option<PathBuf> {
        std::env::var_os("CW_GAME_DIR").map(PathBuf::from)
    }

    /// The whole GameController widget tree from the shipped files, then a few frames of the
    /// menu flow (start menu → character select → creation → world select → world creation →
    /// in game). Skipped without `CW_GAME_DIR`.
    #[test]
    fn game_ui_from_shipped_files() {
        let Some(dir) = game_dir() else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = GamePlxLoader::new(&dir);
        let mut game = GameView::default();
        let mut gui = Gui::new();
        gui.viewport = IVec2::new(1280, 720);
        let mut ui = GameUi::new(gui, &mut plx, &game);
        assert!(plx.errors.is_empty(), "{:?}", plx.errors);
        let files: Vec<&str> = plx.loaded.iter().map(|l| l.file.as_str()).collect();
        assert_eq!(files, ["gui.plx", "quest-tag.plx", "help.plx", "start.plx", "cursor.plx"]);
        let m = ui.m.clone();
        // The named nodes the ctor looks up exist in the shipped files.
        for (name, n) in [
            ("questtag", m.questtag),
            ("smallquesttag", m.smallquesttag),
            ("cubeworld", m.cubeworld),
            ("picroma", m.picroma),
            ("landname", m.landname),
            ("blackwidget", m.blackwidget),
            ("itembox", m.itembox),
            ("itemselector", m.itemselector),
            ("lifebar", m.life_bar),
            ("experiencebar", m.experience_bar),
            ("castbar", m.cast_bar),
            ("crosshair", m.crosshair),
            ("preview", m.preview),
        ] {
            assert!(n.is_some(), "{name} missing");
        }
        assert!(m.target_bars.iter().all(|b| b.is_some()), "target bars: {:?}", m.target_bars);
        // Templates are taken out of the tree (`setParent(NULL)` 0x00636950); the clones made
        // from them are shown and carry the template's widget and a copy of its shape.
        assert!(ui.gui.nodes[m.blackwidget.unwrap()].parent.is_none());
        let back = m.back.unwrap();
        assert!(ui.gui.nodes[back].widget.is_some());
        assert_eq!(m.connections.len(), 12);
        // onResize at 1280x720.
        let gc = m.on_resize_widgets(&ui.gui);
        cw_ui::widget::game_controller_on_resize(&mut ui.gui, &gc, 1280, 720);

        let mut rng = cw_math::rand::MsvcRand::new(1);
        let mut inp = FrameInputs::basic(&mut rng, 1.0, 16, IVec2::new(1280, 720));
        // Frame 1: the start screen with the cursor over "Start Game".
        let sm = m.start_menu.unwrap();
        let origin = ui.gui.widget_world(sm).transform_point2(glam::Vec2::ZERO);
        ui.gui.cursor = origin + glam::Vec2::new(100.0, 25.0);
        let f = ui.frame(&mut game, &mut inp);
        assert_eq!(f.start_menu.len(), 3);
        assert_eq!(ui.start_menu.hovered, 0);
        let a = ui.on_mouse_down(&mut game, 0, true, None);
        assert!(a.is_empty());
        assert!(ui.visible(m.char_select_root) && !ui.visible(m.start_root));
        // Frame 2: character select, "Back" is up and the camera orbits.
        let f = ui.frame(&mut game, &mut inp);
        assert!(ui.visible(m.back));
        assert!(f.actions.contains(&UiAction::OrbitCamera));
        // "Select" with no characters: the creation screen.
        let press = |ui: &mut GameUi, game: &mut GameView, n: Option<NodeId>| {
            let w = ui.gui.nodes[n.unwrap()].widget.unwrap();
            ui.apply(&[UiEvent::Signal { widget: w, event: event::LEFT_RELEASE }], game)
        };
        let a = press(&mut ui, &mut game, m.char_select);
        assert!(a.contains(&UiAction::NewCharacter));
        flow::set_edit_text(&mut ui, m.character_name, "Wollay");
        ui.frame(&mut game, &mut inp);
        assert!(ui.visible(m.create_character));
        let a = press(&mut ui, &mut game, m.create_character);
        assert!(a.contains(&UiAction::SaveCharacter { index: 0 }));
        assert!(ui.visible(m.world_select_root));
        // World select → world creation → in game.
        press(&mut ui, &mut game, m.world_select);
        assert!(ui.visible(m.world_create_root));
        flow::set_edit_text(&mut ui, m.world_name, "Home");
        flow::set_edit_text(&mut ui, m.world_seed, "1234");
        ui.frame(&mut game, &mut inp);
        let a = press(&mut ui, &mut game, m.create_world);
        assert!(a.contains(&UiAction::StartWorld { seed: 1234, name: "Home".into() }));
        game.load_distance = 20.0;
        ui.frame(&mut game, &mut inp);
        assert!(ui.visible(m.gui_root) && !ui.visible(m.back) && !ui.visible(m.wait));
        // In game: B opens the bag; its InventoryWidget draws; C opens crafting.
        flow::toggle_inventory(&mut ui);
        flow::toggle_crafting(&mut ui, &game);
        let f = ui.frame(&mut game, &mut inp);
        assert!(f.bag.as_ref().is_some_and(|b| b.visible || b.texts.is_empty()));
        assert!(f.crafting.is_some() && f.shop.is_none());
        assert!(ui.flag_8008f0);
        // The panels' part nodes came from the shipped templates.
        assert!(m.craft_button.is_some() && m.craft_bar.is_some() && m.enchant_itemframe.is_some());
        assert!(m.adaption_rightarrow.is_some());
    }
}

