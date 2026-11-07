//! [`CameraOverlay`]: camera-aware wrapper for node pop-out overlays.

use super::*;
use iced_widget::core::Transformation;

/// Camera-aware wrapper for one node pop-out overlay (a combo box menu, a
/// tooltip).
///
/// The pop-outs live in ZOOMED-SCREEN space: screen pixels divided by the
/// zoom, with the window's top-left corner at the origin. Node elements lay
/// out in the widget's layout-absolute space, so [`NodeGraph::overlay`] hands
/// them [`Camera2D::overlay_translation`], the offset that carries a layout
/// position into this space, and the window divided by the zoom as the room
/// they may fill; a pop-out anchors at `layout.position() + translation`, as
/// every iced pop-out does, and lays itself out in its constructor against
/// that room. From there the wrapper only has to scale: it draws the content
/// under `scale(zoom)` and maps the screen cursor by `1 / zoom` for
/// hit-testing.
///
/// This is the one space in which iced's own pop-outs place themselves
/// correctly: a menu decides whether it fits below its anchor and how wide it
/// may be from `window - position`, which needs the anchor and the room to
/// share an origin. The layout-absolute space cannot offer that under pan: a
/// node far from the world origin has layout coordinates beyond `window /
/// zoom` however centered it is on screen, and the menu concludes it has no
/// room at all.
///
/// Each pop-out layers itself (`Renderer::with_layer` in its own `draw`), and
/// does so inside this wrapper's transform, so its clip scales with it.
///
/// [`NodeGraph::overlay`]: crate::NodeGraph
/// [`Camera2D::overlay_translation`]: crate::node_graph::camera::Camera2D::overlay_translation
pub(super) struct CameraOverlay<'a, Message, Theme, Renderer> {
    content: overlay::Element<'a, Message, Theme, Renderer>,
    zoom: f32,
}

impl<'a, Message, Theme, Renderer> CameraOverlay<'a, Message, Theme, Renderer>
where
    Renderer: iced_wgpu::core::renderer::Renderer + 'a,
    Message: 'a,
    Theme: 'a,
{
    /// Wraps every pop-out in `content`, each in a wrapper of its own.
    pub(super) fn wrap(
        content: Vec<overlay::Element<'a, Message, Theme, Renderer>>,
        zoom: f32,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        content
            .into_iter()
            .map(|content| overlay::Element::new(Box::new(CameraOverlay { content, zoom })))
            .collect()
    }
}

impl<Message, Theme, Renderer> CameraOverlay<'_, Message, Theme, Renderer> {
    /// Screen cursor to zoomed-screen space.
    fn cursor(&self, cursor: mouse::Cursor) -> mouse::Cursor {
        let inv = 1.0 / self.zoom;
        let map = |p: Point| Point::new(p.x * inv, p.y * inv);
        match cursor {
            mouse::Cursor::Available(p) => mouse::Cursor::Available(map(p)),
            mouse::Cursor::Levitating(p) => mouse::Cursor::Levitating(map(p)),
            mouse::Cursor::Obstructed(p) => mouse::Cursor::Obstructed(map(p)),
            mouse::Cursor::Unavailable => mouse::Cursor::Unavailable,
        }
    }
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for CameraOverlay<'_, Message, Theme, Renderer>
where
    Renderer: iced_wgpu::core::renderer::Renderer,
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        let cursor = self.cursor(cursor);
        renderer.with_transformation(Transformation::scale(self.zoom), |renderer| {
            self.content
                .as_overlay()
                .draw(renderer, theme, style, cursor);
        });
    }

    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let cursor = self.cursor(cursor);
        self.content
            .as_overlay_mut()
            .update(event, cursor, renderer, shell);
    }

    fn mouse_interaction(&self, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        let cursor = self.cursor(cursor);
        self.content
            .as_overlay()
            .mouse_interaction(cursor, renderer)
    }

    fn operate(&mut self, renderer: &Renderer, operation: &mut dyn widget::Operation) {
        self.content.as_overlay_mut().operate(renderer, operation);
    }

    fn overlay<'c>(
        &'c mut self,
        renderer: &Renderer,
    ) -> Vec<overlay::Element<'c, Message, Theme, Renderer>> {
        let zoom = self.zoom;
        let nested = self.content.as_overlay_mut().overlay(renderer);
        nested
            .into_iter()
            .map(|content| overlay::Element::new(Box::new(CameraOverlay { content, zoom })))
            .collect()
    }

    fn index(&self) -> f32 {
        self.content.as_overlay().index()
    }
}
