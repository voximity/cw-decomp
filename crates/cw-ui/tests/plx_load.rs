//! Loads the five shipped `.plx` files (skipped when `CW_GAME_DIR` is unset), builds
//! every shape's drawing buffers and every widget, and checks the widget layout of
//! `start.plx` against a golden file.

use std::fmt::Write as _;
use std::path::PathBuf;

use cw_formats::plx::{self, Item};
use cw_ui::loader::{load, LoadOptions, SceneShape};
use cw_ui::shape::Shape;
use cw_ui::widget::WidgetKind;

fn game_dir() -> Option<PathBuf> {
    std::env::var_os("CW_GAME_DIR").map(PathBuf::from)
}

const FILES: [&str; 5] = ["gui.plx", "start.plx", "help.plx", "quest-tag.plx", "cursor.plx"];

fn kind_name(k: &WidgetKind) -> &'static str {
    match k {
        WidgetKind::Base => "Widget",
        WidgetKind::Button(_) => "Button",
        WidgetKind::PopUpButton(..) => "PopUpButton",
        WidgetKind::ScrollButton(..) => "ScrollButton",
        WidgetKind::ScrollSlider(..) => "ScrollSlider",
        WidgetKind::Edit(_) => "Edit",
        WidgetKind::ListWidget => "ListWidget",
        WidgetKind::Game(_) => "Game",
    }
}

#[test]
fn load_shipped_plx_files() {
    let Some(dir) = game_dir() else {
        eprintln!("CW_GAME_DIR unset; skipping");
        return;
    };
    for f in FILES {
        let bytes = std::fs::read(dir.join(f)).unwrap();
        let doc = plx::parse(&bytes).unwrap();
        let n_nodes = doc.nodes().count();
        let n_shapes = doc.shapes().count();
        let n_tex = doc.items.iter().filter(|i| matches!(i, Item::Texture(_))).count();
        let n_widget_elems = doc.items.iter().filter(|i| matches!(i, Item::Widget(..))).count();
        let (mut gui, scene) = load(&doc, LoadOptions::default());
        // One gui node per Node element (the first is the load target = root).
        assert_eq!(gui.nodes.len(), n_nodes, "{f}");
        assert_eq!(scene.nodes.len(), n_nodes, "{f}");
        assert_eq!(scene.shapes.len(), n_shapes, "{f}");
        assert_eq!(scene.widget_sources.len(), n_widget_elems, "{f}");
        assert!(scene.textures.len() <= n_tex, "{f}");
        let referenced = scene.nodes.iter().skip(1).filter(|n| n.widget_element.is_some()).count();
        assert_eq!(gui.widgets.len(), referenced, "{f}");
        // Every node except the root has a parent, reached from the root.
        let root = gui.root.unwrap();
        for (i, n) in gui.nodes.iter().enumerate() {
            if i != root {
                assert!(n.parent.is_some(), "{f}: node {i} ({}) unlinked", n.name);
            }
        }
        for t in &scene.textures {
            if t.width > 0 && t.height > 0 && !t.pixels.is_empty() {
                let px = (t.width * t.height) as usize;
                assert!(t.pixels.len() == px * 4 || t.pixels.len() == px * 3, "{f}: texture {} size", t.name);
            }
        }
        // Every shape's drawing buffers.
        let mut drawings = 0;
        for s in &scene.shapes {
            if let SceneShape::Mesh(m, _) = &mut *s.borrow_mut() {
                for d in [&mut m.fill, &mut m.stroke, &mut m.extrusion].into_iter().flatten() {
                    let b = d.build_buffers();
                    assert!(b.indices.iter().all(|&i| (i as usize) < b.vertices.len()), "{f}: {}", m.source.name);
                    drawings += 1;
                }
                let _ = m.drawings();
            }
        }
        assert!(drawings > 0 || n_shapes == 0, "{f}");
        // Every widget: update and query its geometry.
        gui.viewport = glam::IVec2::new(1280, 720);
        for w in 0..gui.widgets.len() {
            gui.update(w);
            let s = gui.get_size(w);
            assert!(s.x.is_finite() && s.y.is_finite(), "{f}: widget {}", gui.widgets[w].name);
        }
        // Hit testing works end to end.
        let _ = gui.hit_test(root, glam::Vec2::new(640.0, 360.0), 0);
        eprintln!("{f}: {} nodes, {} widgets, {} shapes, {} textures, {} drawings", gui.nodes.len(), gui.widgets.len(), scene.shapes.len(), scene.textures.len(), drawings);
    }
}

/// Layout dump: every widget (name, class, frame origin, size) and every node (name,
/// parent, world origin, local shape bounds). `start.plx` has no widget elements, so its
/// golden is the node list; `gui.plx` covers the widgets.
fn layout(dir: &std::path::Path, file: &str) -> String {
    let doc = plx::parse(&std::fs::read(dir.join(file)).unwrap()).unwrap();
    let (gui, _scene) = load(&doc, LoadOptions::default());
    let mut out = String::new();
    let _ = writeln!(out, "# widgets: index, name, class, frame origin, size");
    for (i, w) in gui.widgets.iter().enumerate() {
        let p = gui.widget_world(i).translation;
        let s = gui.get_size(i);
        let _ = writeln!(out, "{}\t{}\t{}\t{:?} {:?}\t{:?} {:?}", i, w.name, kind_name(&w.kind), p.x, p.y, s.x, s.y);
    }
    let _ = writeln!(out, "# nodes: index, name, parent, world origin, shape bounds");
    for (i, n) in gui.nodes.iter().enumerate() {
        let o = gui.node_world(i).translation;
        let b = n.shape.as_ref().map(|s| s.bounds());
        let bs = match b {
            Some((lo, hi)) => format!("{:?} {:?} {:?} {:?}", lo.x, lo.y, hi.x, hi.y),
            None => "-".into(),
        };
        let _ = writeln!(out, "{}\t{}\t{:?}\t{:?} {:?}\t{}", i, n.name, n.parent, o.x, o.y, bs);
    }
    out
}

const CR_LF: &str = "\r\n";
const LF: &str = "\n";

fn golden(file: &str, golden_name: &str) {
    let Some(dir) = game_dir() else {
        eprintln!("CW_GAME_DIR unset; skipping");
        return;
    };
    let got = layout(&dir, file);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(golden_name);
    if std::env::var_os("CW_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("golden missing; run with CW_UPDATE_GOLDEN=1");
    assert_eq!(got.replace(CR_LF, LF), want.replace(CR_LF, LF));
}

#[test]
fn start_plx_layout_golden() {
    golden("start.plx", "start_layout.txt");
}

#[test]
fn gui_plx_widget_golden() {
    golden("gui.plx", "gui_layout.txt");
}

#[test]
fn gui_plx_hit_test_finds_widgets() {
    let Some(dir) = game_dir() else {
        return;
    };
    let doc = plx::parse(&std::fs::read(dir.join("gui.plx")).unwrap()).unwrap();
    let (mut gui, _scene) = load(&doc, LoadOptions::default());
    // Make everything visible (the game shows panels on demand), then probe the centre
    // of each widget's frame: the hit node's owner should be that widget or one of its
    // descendants/ancestors in most cases.
    for n in &mut gui.nodes {
        n.visible = true;
    }
    let root = gui.root.unwrap();
    let mut hits = 0;
    for w in 0..gui.widgets.len() {
        let p = gui.widget_world(w).translation + gui.get_size(w) * 0.5;
        if let Some(n) = gui.hit_test(root, p, 0) {
            if gui.owner(n).is_some() {
                hits += 1;
            }
        }
    }
    eprintln!("{hits} of {} widget centres hit a widget", gui.widgets.len());
    assert!(hits > 0);
}

#[test]
fn gui_plx_button_state_animates() {
    let Some(dir) = game_dir() else {
        return;
    };
    let doc = plx::parse(&std::fs::read(dir.join("gui.plx")).unwrap()).unwrap();
    let (mut gui, mut scene) = load(&doc, LoadOptions::default());
    // A mesh with a "button:enter" colour sequence.
    let (node, si) = scene
        .node_objects
        .iter()
        .find_map(|(&n, &(_, _, s))| {
            let s = s?;
            match &*scene.shapes[s].borrow() {
                SceneShape::Mesh(m, _) if m.source.vertex_colors.sequence("button:enter").is_some() => Some((n, s)),
                _ => None,
            }
        })
        .expect("gui.plx has a button:enter colour sequence");
    let before = match &*scene.shapes[si].borrow() {
        SceneShape::Mesh(m, _) => m.source.vertex_colors.current.clone(),
        _ => unreachable!(),
    };
    gui.nodes[node].visible = true;
    let mut cur = Some(node);
    while let Some(c) = cur {
        gui.nodes[c].visible = true;
        cur = gui.nodes[c].parent;
    }
    scene.set_state(&gui, node, "button:enter", 0);
    for _ in 0..30 {
        scene.update(&mut gui, 20);
    }
    let (after, active) = match &*scene.shapes[si].borrow() {
        SceneShape::Mesh(m, a) => (m.source.vertex_colors.current.clone(), *a),
        _ => unreachable!(),
    };
    assert_ne!(before, after, "the colours moved to the sequence's key frame");
    assert!(!active, "the transition ended after its last key");
}

