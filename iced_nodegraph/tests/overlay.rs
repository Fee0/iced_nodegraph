//! Overlay-forwarding tests: NodeGraph must hand each node's pop-out overlay
//! (combo box menu, tooltip, vanilla `menu`, ...) up through `Widget::overlay`,
//! wrapped in the camera transform. The wrapper has three jobs, one test each:
//!
//! 1. forward only when a child actually produces an overlay (else none),
//! 2. draw the pop-out at the same screen pixel as the node content beneath it,
//!    so it anchors to and scales with that node,
//! 3. hand the wrapped overlay a cursor in the space its anchor was given, so
//!    its own hit-testing agrees with where it is drawn.
//!
//! The anchor space is zoomed-screen space (screen pixels over zoom, origin at
//! the window corner): the one space in which iced's pop-outs judge their room
//! correctly, see `CameraOverlay`.
//!
//! Like the sibling recording-renderer tests, these use a fake renderer: the
//! guarantees live in the overlay element's presence, the absolute rect it
//! draws at, and the cursor it receives -- not in pixel output.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use iced::advanced::renderer::Renderer as _;
use iced::advanced::widget::{Meta, Tree, Widget};
use iced::advanced::{Layout, layout, mouse, overlay, renderer, shell};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size, Theme, Vector};

use iced_nodegraph::{Indexed, NodeGraph, PinSide, node, node_pin};

mod common;

use common::record::{Recorded, Recorder};
use common::{headless_shell, mount};

// ---------------------------------------------------------------------------
// An overlay that paints a 10x10 quad at a fixed anchor and records what it was
// handed. The anchor is captured (in layout-absolute space) when the host
// widget produces the overlay, which is also when the overlay lays itself out.
// ---------------------------------------------------------------------------

/// What a probe records about the frame it was drawn in.
#[derive(Default)]
struct ProbeLog {
    /// Anchor the host widget captured: its layout position plus the
    /// translation the graph handed it.
    anchor: Cell<Option<Point>>,
    /// Cursor the overlay's `update` received.
    cursor: Cell<Option<Point>>,
    /// Innermost clip the overlay painted under.
    clip: Cell<Option<Rectangle>>,
    /// `window` the overlay was laid out against in its constructor.
    bounds: Cell<Option<Size>>,
}

struct ProbeOverlay {
    layout: Layout,
    window: Size,
    log: Rc<ProbeLog>,
}

impl ProbeOverlay {
    /// Lays the probe out at `anchor` against `window`, the way iced's
    /// pop-outs lay themselves out in their constructor.
    fn new(anchor: Point, window: Size, log: Rc<ProbeLog>) -> Self {
        log.bounds.set(Some(window));
        Self {
            layout: Layout::new(Size::new(10.0, 10.0)).move_to(anchor),
            window,
            log,
        }
    }
}

impl overlay::Overlay<(), Theme, Recorder> for ProbeOverlay {
    fn draw(
        &self,
        renderer: &mut Recorder,
        _theme: &Theme,
        _style: &renderer::Style,
        _cursor: mouse::Cursor,
    ) {
        // An overlay layers itself; like a tooltip, the probe layers against
        // the room it was handed.
        renderer.with_layer(Rectangle::with_size(self.window), |renderer| {
            self.log.clip.set(renderer.clip());
            renderer.fill_quad(
                renderer::Quad {
                    bounds: self.layout.bounds(),
                    border: iced::Border::default(),
                    shadow: iced::Shadow::default(),
                    snap: false,
                },
                Background::Color(Color::WHITE),
            );
        });
    }
    fn update(
        &mut self,
        _event: &iced::Event,
        cursor: mouse::Cursor,
        _renderer: &Recorder,
        _shell: &mut iced_wgpu::core::Shell<'_, ()>,
    ) {
        self.log.cursor.set(cursor.position());
    }
}

// ---------------------------------------------------------------------------
// A leaf node-content widget that produces `ProbeOverlay` (mirrors combo_box /
// tooltip exposing a pop-out). The anchor it captures is its own layout
// position plus the incoming translation, exactly as the real widgets do.
// ---------------------------------------------------------------------------
struct OverlayProbe {
    log: Rc<ProbeLog>,
}

impl Meta for OverlayProbe {}

impl Widget<(), Theme, Recorder> for OverlayProbe {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(40.0), Length::Fixed(20.0))
    }
    fn layout(&mut self, tree: &mut Tree, _: &Recorder, limits: &layout::Limits) {
        tree.size = layout::atomic(limits, Length::Fixed(40.0), Length::Fixed(20.0));
    }
    fn draw(
        &self,
        _: &Tree,
        _: &mut Recorder,
        _: &Theme,
        _: &renderer::Style,
        _: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }
    fn overlay<'a>(
        &'a mut self,
        _tree: &'a mut Tree,
        layout: Layout,
        _renderer: &Recorder,
        _viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, (), Theme, Recorder>> {
        let anchor = layout.position() + translation;
        self.log.anchor.set(Some(anchor));
        vec![overlay::Element::new(Box::new(ProbeOverlay::new(
            anchor,
            window,
            self.log.clone(),
        )))]
    }
}

// A leaf with no overlay (the trait default returns none).
struct PlainLeaf;
impl Meta for PlainLeaf {}
impl Widget<(), Theme, Recorder> for PlainLeaf {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(40.0), Length::Fixed(20.0))
    }
    fn layout(&mut self, tree: &mut Tree, _: &Recorder, limits: &layout::Limits) {
        tree.size = layout::atomic(limits, Length::Fixed(40.0), Length::Fixed(20.0));
    }
    fn draw(
        &self,
        _: &Tree,
        _: &mut Recorder,
        _: &Theme,
        _: &renderer::Style,
        _: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }
}

const VIEWPORT: Size = Size::new(1024.0, 768.0);

/// The graph shape these tests drive: default ids, recording renderer. Named
/// because the ids come first in `NodeGraph`'s parameter list, so reaching
/// `Recorder` means spelling all four of them.
type RecordedGraph = NodeGraph<'static, Indexed, (), Theme, Recorder>;

/// Lays out a single-node graph at `origin` with the given camera, runs one
/// no-op update so `camera()` syncs into the widget camera, and returns the parts
/// needed to drive `overlay()`: the graph, its tree, and its layout placed at
/// `origin`.
fn graph_with_node(
    origin: Vector,
    node_world: Point,
    camera_pos: Point,
    camera_zoom: f32,
    element: impl Widget<(), Theme, Recorder> + 'static,
    renderer: &Recorder,
) -> (RecordedGraph, Tree, Layout) {
    let mut graph: RecordedGraph = NodeGraph::default()
        .width(Length::Fixed(400.0))
        .height(Length::Fixed(400.0))
        .camera(camera_pos, camera_zoom);
    graph = graph.push_node(node(0usize, node_world, element));

    let (mut tree, layout) = mount(
        &mut graph,
        renderer,
        &layout::Limits::new(Size::ZERO, VIEWPORT),
    );

    // Sync `camera()` into the widget camera (host value differs from the unset
    // last-synced value); the event itself is a no-op. Mirrors the real
    // pipeline, where update() runs before overlay().
    let layout = layout.move_to(Point::ORIGIN + origin);
    let viewport = Rectangle::new(Point::ORIGIN, VIEWPORT);
    let mut bus = shell::Bus::new();
    let mut shell = headless_shell::<()>(&mut bus);
    graph.update(
        &mut tree,
        &iced::Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(-1.0, -1.0),
        }),
        layout,
        mouse::Cursor::Unavailable,
        renderer,
        &mut shell,
        &viewport,
    );

    (graph, tree, layout)
}

/// The single overlay `graph` produces against a `VIEWPORT`-sized window.
fn single_overlay<'a>(
    graph: &'a mut RecordedGraph,
    tree: &'a mut Tree,
    layout: Layout,
    renderer: &Recorder,
) -> overlay::Element<'a, (), Theme, Recorder> {
    let viewport = Rectangle::new(Point::ORIGIN, VIEWPORT);
    let mut overlays = graph.overlay(tree, layout, renderer, &viewport, Vector::ZERO, VIEWPORT);
    assert_eq!(overlays.len(), 1, "exactly one overlay must be present");
    overlays.pop().expect("overlay must be present")
}

fn draw_overlay(ov: &overlay::Element<'_, (), Theme, Recorder>, renderer: &mut Recorder) {
    ov.as_overlay().draw(
        renderer,
        &Theme::Dark,
        &renderer::Style {
            text_color: Color::WHITE,
        },
        mouse::Cursor::Unavailable,
    );
}

#[test]
fn overlay_forwarded_when_child_has_one() {
    let renderer = Recorder::new(Rc::new(RefCell::new(Recorded::default())));
    let (mut graph, mut tree, layout) = graph_with_node(
        Vector::ZERO,
        Point::new(50.0, 50.0),
        Point::ORIGIN,
        1.0,
        OverlayProbe { log: Rc::default() },
        &renderer,
    );
    let viewport = Rectangle::new(Point::ORIGIN, VIEWPORT);

    let ov = graph.overlay(
        &mut tree,
        layout,
        &renderer,
        &viewport,
        Vector::ZERO,
        VIEWPORT,
    );
    assert!(
        !ov.is_empty(),
        "a node whose child produces an overlay must surface it through NodeGraph::overlay",
    );
}

#[test]
fn no_overlay_when_no_child_has_one() {
    let renderer = Recorder::new(Rc::new(RefCell::new(Recorded::default())));
    let (mut graph, mut tree, layout) = graph_with_node(
        Vector::ZERO,
        Point::new(50.0, 50.0),
        Point::ORIGIN,
        1.0,
        PlainLeaf,
        &renderer,
    );
    let viewport = Rectangle::new(Point::ORIGIN, VIEWPORT);

    let ov = graph.overlay(
        &mut tree,
        layout,
        &renderer,
        &viewport,
        Vector::ZERO,
        VIEWPORT,
    );
    assert!(
        ov.is_empty(),
        "with no child overlay, NodeGraph must yield no overlay (not an empty wrapper)",
    );
}

#[test]
fn overlay_draws_through_camera_transform() {
    // The crux: the pop-out must be drawn at the same screen pixel as the node
    // content beneath it -- origin + (world + camera_pos) * zoom -- so it tracks
    // the node under zoom, pan, and a non-zero widget origin.
    let origin = Vector::new(0.0, 100.0);
    let world = Point::new(30.0, 40.0);
    let cam_pos = Point::new(20.0, -10.0);
    let zoom = 2.0;

    let out = Rc::new(RefCell::new(Recorded::default()));
    let mut renderer = Recorder::new(out.clone());
    let (mut graph, mut tree, layout) = graph_with_node(
        origin,
        world,
        cam_pos,
        zoom,
        OverlayProbe { log: Rc::default() },
        &renderer,
    );

    let ov = single_overlay(&mut graph, &mut tree, layout, &renderer);
    draw_overlay(&ov, &mut renderer);

    let drawn = out
        .borrow()
        .quads
        .first()
        .copied()
        .expect("overlay drew a quad");
    let expected = Point::new(
        origin.x + (world.x + cam_pos.x) * zoom,
        origin.y + (world.y + cam_pos.y) * zoom,
    );
    assert!(
        (drawn.x - expected.x).abs() < 0.5 && (drawn.y - expected.y).abs() < 0.5,
        "overlay drawn at {drawn:?} should sit at {expected:?} (origin {origin:?}, zoom {zoom})",
    );
    // The pop-out scales with the camera too: a 10px anchor box -> 10 * zoom.
    assert!(
        (drawn.width - 10.0 * zoom).abs() < 0.5,
        "overlay should scale with zoom: width {} expected {}",
        drawn.width,
        10.0 * zoom,
    );
}

#[test]
fn overlay_of_nested_graph_composes_both_cameras() {
    // A graph inside a node: the pop-out of the inner node must be drawn where
    // the inner content is, which is the inner camera applied first and the
    // outer camera on top -- origin + (P_o + N_o + Z_i * (N_i + P_i)) * Z_o.
    let origin = Vector::new(0.0, 100.0);
    let outer_world = Point::new(30.0, 40.0);
    let outer_cam = Point::new(20.0, -10.0);
    let outer_zoom = 2.0;
    let inner_world = Point::new(11.0, 13.0);
    let inner_cam = Point::new(5.0, 7.0);
    let inner_zoom = 1.5;

    let out = Rc::new(RefCell::new(Recorded::default()));
    let mut renderer = Recorder::new(out.clone());
    let inner: RecordedGraph = NodeGraph::default()
        .width(Length::Fixed(200.0))
        .height(Length::Fixed(200.0))
        .camera(inner_cam, inner_zoom)
        .push_node(node(
            0usize,
            inner_world,
            OverlayProbe { log: Rc::default() },
        ));
    let (mut graph, mut tree, layout) =
        graph_with_node(origin, outer_world, outer_cam, outer_zoom, inner, &renderer);

    let ov = single_overlay(&mut graph, &mut tree, layout, &renderer);
    draw_overlay(&ov, &mut renderer);

    let drawn = out
        .borrow()
        .quads
        .first()
        .copied()
        .expect("overlay drew a quad");
    let expected = Point::new(
        origin.x
            + (outer_cam.x + outer_world.x + inner_zoom * (inner_world.x + inner_cam.x))
                * outer_zoom,
        origin.y
            + (outer_cam.y + outer_world.y + inner_zoom * (inner_world.y + inner_cam.y))
                * outer_zoom,
    );
    assert!(
        (drawn.x - expected.x).abs() < 0.5 && (drawn.y - expected.y).abs() < 0.5,
        "nested overlay drawn at {drawn:?} should sit at {expected:?}",
    );
    let expected_width = 10.0 * inner_zoom * outer_zoom;
    assert!(
        (drawn.width - expected_width).abs() < 0.5,
        "nested overlay should scale with both zooms: width {} expected {expected_width}",
        drawn.width,
    );
}

#[test]
fn overlay_receives_the_cursor_in_its_anchor_space() {
    // Round trip: a screen cursor placed where the overlay anchor draws must
    // reach the wrapped overlay AS that anchor, whatever space the graph put
    // the anchor in -- the pop-out hit-tests the cursor against its own
    // layout, so the two must agree.
    let origin = Vector::new(0.0, 100.0);
    let world = Point::new(30.0, 40.0);
    let cam_pos = Point::new(20.0, -10.0);
    let zoom = 2.0;

    let log = Rc::new(ProbeLog::default());
    let renderer = Recorder::new(Rc::new(RefCell::new(Recorded::default())));
    let (mut graph, mut tree, layout) = graph_with_node(
        origin,
        world,
        cam_pos,
        zoom,
        OverlayProbe { log: log.clone() },
        &renderer,
    );

    let mut ov = single_overlay(&mut graph, &mut tree, layout, &renderer);

    // Screen pixel where the anchor draws (same mapping as the draw test).
    let screen = Point::new(
        origin.x + (world.x + cam_pos.x) * zoom,
        origin.y + (world.y + cam_pos.y) * zoom,
    );
    let mut bus = shell::Bus::new();
    let mut shell = headless_shell::<()>(&mut bus);
    ov.as_overlay_mut().update(
        &iced::Event::Mouse(mouse::Event::CursorMoved { position: screen }),
        mouse::Cursor::Available(screen),
        &renderer,
        &mut shell,
    );

    let seen = log.cursor.get().expect("overlay must receive a cursor");
    let anchor = log.anchor.get().expect("probe captured its anchor");
    assert!(
        (seen.x - anchor.x).abs() < 0.5 && (seen.y - anchor.y).abs() < 0.5,
        "cursor reached overlay as {seen:?}, expected its anchor {anchor:?}",
    );
}

#[test]
fn overlay_anchored_on_screen_has_room_under_pan() {
    // A node far from the world origin, panned onto the screen: its layout
    // coordinates exceed `window / zoom`, so an anchor left in layout space
    // would tell a menu it has negative room to the right and below. The
    // anchor must land inside the window the pop-out is laid out against.
    let world = Point::new(3000.0, 2000.0);
    let cam_pos = Point::new(-2800.0, -1900.0); // node at screen (200, 100)
    let zoom = 1.0;

    let log = Rc::new(ProbeLog::default());
    let renderer = Recorder::detached();
    let (mut graph, mut tree, layout) = graph_with_node(
        Vector::ZERO,
        world,
        cam_pos,
        zoom,
        OverlayProbe { log: log.clone() },
        &renderer,
    );

    let _ov = single_overlay(&mut graph, &mut tree, layout, &renderer);

    let anchor = log.anchor.get().expect("probe captured its anchor");
    let bounds = log.bounds.get().expect("overlay must be laid out");
    assert!(
        (anchor.x - 200.0).abs() < 0.5 && (anchor.y - 100.0).abs() < 0.5,
        "anchor {anchor:?} should be where the node is on screen, (200, 100)",
    );
    assert!(
        anchor.x < bounds.width && anchor.y < bounds.height,
        "anchor {anchor:?} must lie inside the room {bounds:?} the pop-out is given",
    );
}

#[test]
fn overlay_survives_its_own_layer_at_zoom() {
    // An overlay clips itself: it opens a layer (`Renderer::with_layer`) in its
    // own `draw`, against the room it was laid out in, and `push_clip` bakes in
    // the transformation active at entry and REPLACES the parent clip
    // (iced_graphics `layer.rs`). That layer is opened inside the wrapper's
    // camera transform, so it has to scale with it: a room left at
    // `window / zoom` in screen pixels would clip the pop-out to a fraction of
    // the screen. Probe placed so its transformed rect is inside the window but
    // outside `window / zoom` - exactly the band such a clip would lose.
    let world = Point::new(300.0, 200.0);
    let zoom = 2.0;

    let log = Rc::new(ProbeLog::default());
    let out = Rc::new(RefCell::new(Recorded::default()));
    let mut renderer = Recorder::new(out.clone());
    let (mut graph, mut tree, layout) = graph_with_node(
        Vector::ZERO,
        world,
        Point::ORIGIN,
        zoom,
        OverlayProbe { log: log.clone() },
        &renderer,
    );

    let ov = single_overlay(&mut graph, &mut tree, layout, &renderer);
    draw_overlay(&ov, &mut renderer);

    let drawn = out
        .borrow()
        .quads
        .first()
        .copied()
        .expect("overlay drew a quad");
    let clip = log.clip.get().expect("overlay painted inside a clip");
    let room = log.bounds.get().expect("overlay must be laid out");
    assert!(
        clip.intersection(&drawn).is_some_and(|visible| {
            (visible.width - drawn.width).abs() < 0.5 && (visible.height - drawn.height).abs() < 0.5
        }),
        "the pop-out at {drawn:?} is clipped by {clip:?} (its room was {room:?}): \
         the whole quad must survive",
    );
}

#[test]
fn overlay_lays_out_in_layout_units() {
    // The content lays out in layout-absolute space while `window` arrives in
    // screen pixels, so a menu deciding whether it fits below its anchor has to
    // be handed the region divided by zoom - otherwise at zoom 2 it believes it
    // has twice the room it has.
    let zoom = 2.0;
    let log = Rc::new(ProbeLog::default());
    let renderer = Recorder::detached();
    let (mut graph, mut tree, layout) = graph_with_node(
        Vector::ZERO,
        Point::new(30.0, 40.0),
        Point::ORIGIN,
        zoom,
        OverlayProbe { log: log.clone() },
        &renderer,
    );

    let _ov = single_overlay(&mut graph, &mut tree, layout, &renderer);

    let seen = log.bounds.get().expect("overlay must be laid out");
    let expected = Size::new(VIEWPORT.width / zoom, VIEWPORT.height / zoom);
    assert!(
        (seen.width - expected.width).abs() < 0.5 && (seen.height - expected.height).abs() < 0.5,
        "content overlay laid out against {seen:?}, expected {expected:?} at zoom {zoom}",
    );
}

#[test]
fn overlay_of_a_pin_arrives_where_its_content_sits() {
    // A pick list wrapped in a pin: the pin is a wrapper around node content,
    // so its pop-out has to reach the graph at all, and at the same screen
    // pixel the same content reaches it unwrapped. Without the pin forwarding
    // `overlay`, the menu never opens.
    let origin = Vector::new(0.0, 100.0);
    let world = Point::new(30.0, 40.0);
    let cam_pos = Point::new(20.0, -10.0);
    let zoom = 2.0;

    let popout_rect = |element: Element<'static, (), Theme, Recorder>| -> Rectangle {
        let out = Rc::new(RefCell::new(Recorded::default()));
        let mut renderer = Recorder::new(out.clone());
        let (mut graph, mut tree, layout) =
            graph_with_node(origin, world, cam_pos, zoom, element, &renderer);

        let viewport = Rectangle::new(Point::ORIGIN, VIEWPORT);
        let mut overlays = graph.overlay(
            &mut tree,
            layout,
            &renderer,
            &viewport,
            Vector::ZERO,
            VIEWPORT,
        );
        let ov = overlays
            .pop()
            .expect("a pop-out inside the node must reach the graph");
        draw_overlay(&ov, &mut renderer);
        out.borrow()
            .quads
            .first()
            .copied()
            .expect("overlay drew a quad")
    };

    let bare = popout_rect(Widget::_boxed(OverlayProbe { log: Rc::default() }));
    let pinned = popout_rect(Widget::_boxed(node_pin(
        PinSide::Left,
        0usize,
        OverlayProbe { log: Rc::default() },
    )));
    assert_eq!(
        pinned, bare,
        "a pop-out inside a pin must land where the same content lands unwrapped",
    );
}
