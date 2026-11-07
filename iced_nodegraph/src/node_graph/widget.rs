//! Widget implementation for NodeGraph.
//!
//! This module implements the Iced `Widget` trait for [`NodeGraph`], handling:
//! - Layout computation for nodes and their content
//! - Event processing (mouse, keyboard)
//! - SDF-based rendering via iced_nodegraph_sdf primitives
//!
//! ## Rendering Layers
//!
//! The widget renders in four tiers, back to front:
//! 1. Solid background color.
//! 2. Graph background: ONE batched SDF draw under all nodes, internally
//!    ordered grid (z0), node + edge shadows (z1), edge strokes (z2) and
//!    anchors (z3). The hover glow and the edge being dragged follow it, each
//!    as a small primitive of its own.
//! 3. Per node, composited by Iced in z-order: node background (fill) -> node
//!    content (Iced widgets) -> node foreground (border + pins). Embedding Iced
//!    widgets between the two SDF node layers lets nodes overlap correctly.
//! 4. Graph foreground: interaction tools (selection box, edge-cutting overlay)
//!    and the minimap.
//!
//! ## Interface contract
//!
//! What each `Widget` method forwards to the node elements, and in which
//! space. Every forward maps the cursor and viewport through
//! [`Camera2D`](super::camera::Camera2D) into the widget's layout-absolute
//! space, because that is the space the child layouts were produced in.
//!
//! - `layout` measures every node with loose limits and positions it at the
//!   host's world coordinate, so the layout tree is layout-absolute and the
//!   camera applies at draw and input time only.
//! - `update` and `operate` reach every node, offscreen ones included. An
//!   unfocus-on-outside-click, a child drag still in flight, and a focus
//!   traversal or scroll_to target all live on nodes the viewport does not
//!   cover.
//! - `draw` bounds-culls only the SDF layers it builds itself. Node content is
//!   walked for every node: the per-node pre-passes must run regardless, since
//!   edges anchor to pins of offscreen nodes and the background batch has to
//!   stay entry-stable, and an offscreen node's content records into a
//!   zero-area clip. Culling the child walk would buy little and make what is
//!   walked diverge from what is drawn.
//! - `mouse_interaction` recurses into the topmost node under the cursor only.
//!   One cursor wins, so an occluded node must not claim it.
//! - `overlay` collects each node's pop-out and wraps the group in
//!   `CameraOverlay`, which applies the same transform as the node content
//!   beneath it.

use iced_wgpu::core::{
    Layout, Shell, Widget, layout, mouse, overlay, renderer,
    widget::{self, Meta, Tree, tree},
};
use iced_widget::core::{Element, Event, Length, Point, Rectangle, Size, Vector, keyboard};
use web_time::Instant;

use super::{
    ANCHOR_GRAB_THRESHOLD, Counts, DragInfo, EDGE_END_GRAB_LENGTH, EDGE_GRAB_THRESHOLD, Edge,
    GraphInfo, MIN_NODE_SIZE, NodeGraph, OpTiming, RESIZE_GRIP_SIDE,
    cable::{CableGeometry, PendingRoute, PhantomKind, RoutePhantom, Station},
    edge_path,
    euclid::{IntoIced, LayoutVector},
    state::{CameraTween, Dragging, NodeGraphState, PressTarget, z_render_indices},
};
use crate::{
    PinDirection, PinRef, PinSide,
    ids::Ids,
    node_graph::euclid::{IntoEuclid, LayoutPoint, ScreenPoint, WorldPoint},
    node_graph::focus::FocusRequest,
    node_pin::{NodePinState, PinEnd, PinInfo, PinSlot},
    style::{
        AnchorStatus, AnchorStyle, Catalog, EdgeGeometry, EdgeStatus, EdgeStyle, NodeStatus,
        NodeStyle, PinStatus, PinStyle, TilingKind,
    },
};
use iced_nodegraph_sdf::{Pattern, SdfPrimitive, Shape, Style, Tiling};

mod camera_overlay;
mod draw;
mod minimap;
pub(crate) mod update;

use camera_overlay::CameraOverlay;

impl<I, Message, Theme, Renderer> Meta for NodeGraph<'_, I, Message, Theme, Renderer>
where
    I: Ids,
    Theme: Catalog,
{
}

impl<I, Message, Theme, Renderer> iced_wgpu::core::Widget<Message, Theme, Renderer>
    for NodeGraph<'_, I, Message, Theme, Renderer>
where
    I: Ids,
    Theme: Catalog,
    Renderer: iced_wgpu::core::renderer::Renderer + iced_wgpu::primitive::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<NodeGraphState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(NodeGraphState::default())
    }

    fn size(&self) -> Size<Length> {
        self.size
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let size = layout::atomic(limits, self.size.width, self.size.height);
        // Unbounded limits so a node shrinks to fit its content rather than
        // filling the graph. Both axes are flagged infinite, not merely given
        // an infinite `max`: on an infinite axis `Length::Fill` resolves to the
        // intrinsic size, so a `Fill` child (a `node_header`, say) stretches to
        // its widest sibling. With a plain infinite `max` it would resolve to
        // infinity, since iced's containers default to the non-compressing
        // `Length::Fit`.
        //
        // A widget that still sizes itself to the unbounded limits leaves the
        // body with no drawable rectangle, so debug builds name the node here
        // rather than fail three crates down in the SDF boolean over NaN
        // geometry.
        let node_limits = layout::Limits::with_flags(
            Size::ZERO,
            Size::ZERO,
            Size::new(false, false),
            Size::new(true, true),
        );
        for (node_index, ((position, element), node_tree)) in
            self.elements_iter_mut().zip(&mut tree.children).enumerate()
        {
            element.layout(node_tree, renderer, &node_limits);
            debug_assert!(
                node_tree.size.width.is_finite() && node_tree.size.height.is_finite(),
                "node {node_index} body resolved to a non-finite size {:?}: a widget inside the node body sized itself to the unbounded node limits; give it a fixed size",
                node_tree.size,
            );
            node_tree.translation = Vector::new(position.x, position.y);
        }
        tree.size = size;
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.draw_impl(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn diff(&mut self, tree: &mut Tree) {
        let mut children: Vec<&mut Element<'_, Message, Theme, Renderer>> =
            self.elements_iter_mut().map(|(_, e)| e).collect();
        tree.diff_children(&mut children);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        // A `focus` task addressed to this graph leaves its request in the
        // slot. The target is resolved against this layout now and the fit
        // starts on the next update, which is where a shell to commit
        // through exists.
        let mut slot: Option<FocusRequest<I>> = None;
        operation.custom(self.id.as_ref(), layout.bounds(), &mut slot);
        if let Some(request) = slot {
            let state = tree.state.downcast_mut::<NodeGraphState>();
            state.pending_focus =
                update::resolve_focus_target(self, layout, &tree.children, state, &request.target)
                    .map(|world_aabb| (world_aabb, request.options));
        }

        // The node content lives in layout-absolute space, so the viewport it
        // is handed is the camera-inverted one, clipped to the graph first as
        // `draw` clips it - the same space the `update` walk uses.
        let camera = tree
            .state
            .downcast_ref::<NodeGraphState>()
            .camera_for(layout);
        let clipped_viewport = layout
            .bounds()
            .intersection(viewport)
            .unwrap_or(Rectangle::new(layout.bounds().position(), Size::ZERO));
        operation.container(self.id.as_ref(), layout.bounds(), &clipped_viewport);
        camera.update_with(
            &clipped_viewport,
            mouse::Cursor::Unavailable,
            |child_viewport, _| {
                operation.traverse(&mut |operation| {
                    for ((_, element), (node_layout, node_tree)) in self
                        .elements_iter_mut()
                        .zip(layout.iter_mut(&mut tree.children))
                    {
                        element.operate(
                            node_tree,
                            node_layout,
                            child_viewport,
                            renderer,
                            operation,
                        );
                    }
                });
            },
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        // Iced collects pop-out widgets (combo box menus, tooltips, vanilla
        // `menu`) only through `Widget::overlay`. Without forwarding it to the
        // node elements, their underlying widgets draw fine but the pop-out
        // never appears.
        let state = tree.state.downcast_ref::<NodeGraphState>();
        let camera = state.camera_for(layout);

        // Collect each node's overlays (most have none). Child layouts are in
        // the widget's layout-absolute space; the translation carries their
        // anchors into zoomed-screen space, where each pop-out lays itself out
        // against the window divided by zoom and `CameraOverlay` scales it
        // (see its docs for why that space). A parent that itself translates
        // (a scrollable, an enclosing graph) passes that offset in; it
        // composes here.
        //
        // The incoming translation is in the parent's zoomed-screen space; the
        // content is laid out in this widget's layout space and scaled by
        // `zoom` in `CameraOverlay`, so the parent offset enters divided by zoom.
        let zoom = camera.zoom();
        let translation = camera.overlay_translation() + translation * (1.0 / zoom);
        let room = Size::new(window.width / zoom, window.height / zoom);
        let children: Vec<overlay::Element<'b, Message, Theme, Renderer>> = self
            .nodes
            .iter_mut()
            .map(|node| &mut node.element)
            .zip(layout.iter_mut(&mut tree.children))
            .flat_map(|(element, (node_layout, node_tree))| {
                element.overlay(
                    node_tree,
                    node_layout,
                    renderer,
                    viewport,
                    translation,
                    room,
                )
            })
            .collect();

        CameraOverlay::wrap(children, zoom)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        screen_cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.update_impl(
            tree,
            event,
            layout,
            screen_cursor,
            renderer,
            shell,
            viewport,
        );
    }

    /// The cursor the graph claims, in precedence order: an in-flight gesture,
    /// then a node's resize grip or content, then an anchor core or cable below
    /// the nodes.
    ///
    /// Recursion is gated on node bounds instead of forwarded to every child:
    /// only one cursor can win, so an occluded node must not claim it. The
    /// topmost node under the cursor consumes the query even when its subtree
    /// reports [`mouse::Interaction::None`], which is what keeps a node body
    /// from showing a cursor set by something behind it.
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<NodeGraphState>();
        // A gesture in flight owns the cursor wherever it has been dragged to,
        // grip or node bounds left behind: the drag is still going.
        match &state.dragging {
            Dragging::Resize { .. } => return mouse::Interaction::ResizingDiagonallyDown,
            Dragging::Graph(_)
            | Dragging::Node { .. }
            | Dragging::GroupMove { .. }
            | Dragging::Anchor { .. }
            | Dragging::Route { .. }
            | Dragging::RouteOver { .. }
            | Dragging::PressPending { .. } => return mouse::Interaction::Grabbing,
            // The map is chrome the gesture steers from, not content it drags,
            // so the cursor stays the one that offered the click.
            Dragging::Minimap => return mouse::Interaction::Pointer,
            Dragging::Edge { .. }
            | Dragging::EdgeOver { .. }
            | Dragging::EdgeCutting { .. }
            | Dragging::SelectionBox(..) => return mouse::Interaction::Crosshair,
            Dragging::None => {}
        }
        // Outside the graph - or levitating, because a sibling in a `stack`
        // claimed the event - nothing here may claim the cursor.
        if cursor.position_over(layout.bounds()).is_none() {
            return mouse::Interaction::None;
        }
        // The map draws over everything, so it answers the cursor before any
        // node does - and in screen space, the space it was placed in.
        if let Some(minimap) = self.minimap.as_ref()
            && cursor
                .position_over(minimap::rect(minimap, layout.bounds()))
                .is_some()
        {
            return mouse::Interaction::Pointer;
        }
        let camera = state.camera_for(layout);
        // The spaces the child `update` walk uses: cursor and viewport
        // camera-inverted into layout-absolute space, the viewport first
        // clipped to the graph as `draw` clips it.
        let clipped_viewport = layout
            .bounds()
            .intersection(viewport)
            .unwrap_or(Rectangle::new(layout.bounds().position(), Size::ZERO));
        camera.update_with(
            &clipped_viewport,
            cursor,
            |child_viewport, layout_cursor| {
                let Some(position) = layout_cursor.position() else {
                    return mouse::Interaction::None;
                };
                let selection = self.resolved_selection(state);
                let z_indices = z_render_indices(
                    state,
                    self.nodes.len(),
                    |i| selection.contains(&i),
                    |i| self.nodes[i].frame,
                );
                // Top-first, like the press hit-test: a node covering another
                // node's corner takes the cursor with it.
                for &node_index in z_indices.iter().rev() {
                    let Some(node) = self.nodes.get(node_index) else {
                        continue;
                    };
                    let Some(child_tree) = tree.children.get(node_index) else {
                        continue;
                    };
                    let Some(node_layout) = node_layout(layout, &tree.children, node_index) else {
                        continue;
                    };
                    if !node_layout.bounds().contains(position) {
                        continue;
                    }
                    // Grips live in layout-absolute space, the space `update`
                    // hit-tests in, and sit above the node's own content.
                    if self.on_resize.is_some()
                        && node.resizable
                        && resize_grip_zone(node_layout.bounds(), camera.zoom()).contains(position)
                    {
                        return mouse::Interaction::ResizingDiagonallyDown;
                    }
                    return node.element.mouse_interaction(
                        child_tree,
                        node_layout,
                        layout_cursor,
                        child_viewport,
                        renderer,
                    );
                }
                let at: LayoutPoint = position.into_euclid();
                if self.anchor_core_at(tree, at).is_some()
                    || self.cable_hit_at(tree, layout, at).is_some()
                {
                    mouse::Interaction::Grab
                } else {
                    mouse::Interaction::None
                }
            },
        )
    }
}

/// The layout of node `node_index`: the graph's layout moved by the node
/// tree's translation, which `layout` set to the node's world position.
pub(super) fn node_layout(
    layout: Layout,
    node_trees: &[Tree],
    node_index: usize,
) -> Option<Layout> {
    layout
        .iter(node_trees)
        .nth(node_index)
        .map(|(node_layout, _)| node_layout)
}

/// Every node's layout, in push order. See [`node_layout`].
pub(super) fn node_layouts(
    layout: Layout,
    node_trees: &[Tree],
) -> impl DoubleEndedIterator<Item = Layout> + ExactSizeIterator + '_ {
    layout.iter(node_trees).map(|(node_layout, _)| node_layout)
}

/// A pin as found in the laid-out widget tree: its positional index within the
/// owning node, its state, and the `(start, end)` anchors an edge attaches to.
///
/// The index is the pin's position in `find_pins` walk order, which is the
/// `pin_index` the [`Dragging`] states carry.
pub(super) type PinLayout<'a, I> = (
    usize,
    &'a NodePinState<<I as Ids>::PinId, <I as Ids>::Payload>,
    (Point, Point),
);

/// Every pin in a node's subtree, in depth-first layout order.
///
/// The walk does not enter a nested `NodeGraph`; its pins are its own.
///
/// Every pin registers the same [`PinSlot`] tag; the slot's state is the
/// pin's own `NodePinState<PinId, Payload>`. A pin over other types is a
/// debug-build assertion and is skipped in release builds, where it would
/// otherwise be indistinguishable from a node without pins.
fn find_pins<'a, I: Ids>(tree: &'a Tree, layout: Layout) -> Vec<PinLayout<'a, I>> {
    let mut flat = Vec::new();
    let mut pin_index = 0;
    inner_find_pins::<I>(&mut flat, &mut pin_index, layout, tree);
    flat
}

fn inner_find_pins<'a, I: Ids>(
    flat: &mut Vec<PinLayout<'a, I>>,
    pin_index: &mut usize,
    node_layout: Layout,
    pin_tree: &'a Tree,
) {
    // A nested graph's pins belong to that graph. Stop at its tree so the
    // outer graph neither adopts them nor asserts on their id types.
    if pin_tree.tag == tree::Tag::of::<NodeGraphState>() {
        return;
    }

    // iced's single-child wrappers (`container`, `pin`, `float`, `themer`)
    // report their content's tag and a fresh copy of its state as their own,
    // and keep the content as their child. A pin wrapped that way leaves a
    // `PinSlot` tree above the real one whose state is never updated; it is a
    // forwarder, not a pin, and counting it would draw a ghost pin at the
    // node's origin and shift every later pin index.
    let forwards_a_pin = pin_tree
        .children
        .first()
        .is_some_and(|child| child.tag == tree::Tag::of::<PinSlot>());

    if pin_tree.tag == tree::Tag::of::<PinSlot>() && !forwards_a_pin {
        let slot = pin_tree.state.downcast_ref::<PinSlot>();
        match slot.get::<I::PinId, I::Payload>() {
            Some(pin_state) => {
                let node_bounds = node_layout.bounds();
                let pin_positions = pin_positions(pin_state, node_bounds);
                flat.push((*pin_index, pin_state, pin_positions));
                *pin_index += 1;
            }
            None => debug_assert!(
                false,
                "pin state {} inside a graph over {}: a pin's id and payload types must be the graph's Ids::PinId and Ids::Payload",
                slot.type_name(),
                std::any::type_name::<NodePinState<I::PinId, I::Payload>>(),
            ),
        }
    }

    for child_tree in &pin_tree.children {
        inner_find_pins::<I>(flat, pin_index, node_layout, child_tree);
    }
}

/// Orients a connected pair so the OUTPUT pin is `from` (output -> input),
/// independent of which side the drag started on. Mirrors the edge-rendering
/// normalization (`swap` in `draw`), so the endpoints reported to
/// `on_connect`/`on_disconnect` match the visual data-flow direction. Order is
/// only swapped when `from` is a non-output and `to` is an output.
fn orient_connection<I: Ids>(
    from_dir: PinDirection,
    to_dir: PinDirection,
    from: PinRef<I>,
    to: PinRef<I>,
) -> (PinRef<I>, PinRef<I>) {
    let swap = !matches!(from_dir, PinDirection::Output) && matches!(to_dir, PinDirection::Output);
    if swap { (to, from) } else { (from, to) }
}

/// Where an edge attaches to a pin, as (start, end) anchors on the node border.
///
/// Both anchors coincide for a pin on one side; a [`PinSide::Row`] pin spans the
/// node and offers a left and a right anchor, so an edge can arrive on either
/// side of it.
fn pin_positions<P, UI>(state: &NodePinState<P, UI>, node_bounds: Rectangle) -> (Point, Point) {
    let Point { x, y } = state.position;
    let (left, right) = (node_bounds.x, node_bounds.x + node_bounds.width);
    let (top, bottom) = (node_bounds.y, node_bounds.y + node_bounds.height);
    let both = |p: Point| (p, p);
    match state.side {
        PinSide::Row => (Point::new(left, y), Point::new(right, y)),
        PinSide::Left => both(Point::new(left, y)),
        PinSide::Right => both(Point::new(right, y)),
        PinSide::Top => both(Point::new(x, top)),
        PinSide::Bottom => both(Point::new(x, bottom)),
    }
}

/// The bottom-right square a resizable node is grabbed by, in the same
/// layout-absolute space as `bounds`.
///
/// [`RESIZE_GRIP_SIDE`] is a screen-pixel size divided by zoom here, like
/// [`PIN_CLICK_THRESHOLD`](crate::node_graph::PIN_CLICK_THRESHOLD), so the grip
/// covers the same pixels at every zoom. Capped at half the node in each axis:
/// a node at [`MIN_NODE_SIZE`](crate::node_graph::MIN_NODE_SIZE) must still
/// have a body left to drag it by.
///
/// One source for both the hit test and the drawn affordance, so the grip can
/// never be painted somewhere the press does not accept.
fn resize_grip_zone(bounds: Rectangle, zoom: f32) -> Rectangle {
    let side = (RESIZE_GRIP_SIDE / zoom)
        .min(bounds.width * 0.5)
        .min(bounds.height * 0.5);
    Rectangle {
        x: bounds.x + bounds.width - side,
        y: bounds.y + bounds.height - side,
        width: side,
        height: side,
    }
}

#[cfg(test)]
mod find_pins_tests {
    use super::find_pins;
    use crate::{Indexed, PinSide, node_pin};
    use iced::widget::{column, container, text};
    use iced_wgpu::core::{Layout, Size, Widget, widget::Tree};

    fn pins_in(mut content: impl Widget<(), iced::Theme, iced::Renderer>) -> usize {
        let mut tree = Tree::empty();
        tree.diff(&mut content);
        find_pins::<Indexed>(&tree, Layout::new(Size::ZERO)).len()
    }

    // A container reports its content's tag and state as its own: the copy it
    // holds is not a second pin.
    #[test]
    fn a_wrapped_pin_is_found_once() {
        let pin = || node_pin(PinSide::Left, 0usize, text("a"));
        assert_eq!(pins_in(container(pin())), 1);
        assert_eq!(pins_in(container(container(pin()))), 1);
    }

    #[test]
    fn sibling_pins_are_each_found() {
        assert_eq!(
            pins_in(column![
                container(node_pin(PinSide::Left, 0usize, text("a"))),
                node_pin(PinSide::Right, 1usize, text("b")),
            ]),
            2,
        );
    }
}

#[cfg(test)]
mod grip_tests {
    use super::{MIN_NODE_SIZE, resize_grip_zone};
    use iced_widget::core::{Point, Rectangle, Size};

    fn zone(size: Size, zoom: f32) -> Rectangle {
        resize_grip_zone(Rectangle::new(Point::new(10.0, 20.0), size), zoom)
    }

    #[test]
    fn sits_in_the_bottom_right_corner() {
        assert_eq!(
            zone(Size::new(100.0, 60.0), 1.0),
            Rectangle::new(Point::new(98.0, 68.0), Size::new(12.0, 12.0)),
        );
    }

    // The side scales with 1/zoom, so the grip covers the same screen pixels
    // however far the camera is zoomed in or out.
    #[test]
    fn scales_inversely_with_zoom() {
        assert_eq!(zone(Size::new(100.0, 60.0), 2.0).width, 6.0);
        assert_eq!(zone(Size::new(100.0, 60.0), 0.5).width, 24.0);
    }

    // Half the node is the ceiling in each axis: a node at the minimum size
    // keeps a body to drag it by, and a zoomed-out node is not all grip.
    #[test]
    fn never_covers_more_than_half_the_node() {
        assert_eq!(zone(MIN_NODE_SIZE, 1.0).width, 12.0);
        assert_eq!(zone(Size::new(100.0, 60.0), 0.25).width, 30.0);
    }
}

#[cfg(test)]
mod orient_tests {
    use super::orient_connection;
    use crate::node_pin::PinDirection;
    use crate::{Indexed, PinRef};

    // A drag from an output pin to an input pin keeps (output, input) order.
    #[test]
    fn output_to_input_keeps_order() {
        let out = PinRef::<Indexed>::new(0, 0);
        let inp = PinRef::new(1, 0);
        let (from, to) = orient_connection(PinDirection::Output, PinDirection::Input, out, inp);
        assert_eq!(from, PinRef::new(0, 0));
        assert_eq!(to, PinRef::new(1, 0));
    }

    // A drag from an input pin to an output pin is flipped to (output, input),
    // so on_connect reports the same pair regardless of drag direction.
    #[test]
    fn input_to_output_is_flipped() {
        let inp = PinRef::<Indexed>::new(1, 0);
        let out = PinRef::new(0, 0);
        let (from, to) = orient_connection(PinDirection::Input, PinDirection::Output, inp, out);
        assert_eq!(from, PinRef::new(0, 0));
        assert_eq!(to, PinRef::new(1, 0));
    }

    // Ambiguous pairs (Both) are left in drag order; only a non-output -> output
    // pair is swapped.
    #[test]
    fn both_keeps_drag_order() {
        let a = PinRef::<Indexed>::new(0, 0);
        let b = PinRef::new(1, 0);
        let (from, to) = orient_connection(PinDirection::Both, PinDirection::Both, a, b);
        assert_eq!(from, PinRef::new(0, 0));
        assert_eq!(to, PinRef::new(1, 0));
    }
}
