//! Shared harnesses for the test binaries: [`record`] is a fake renderer for
//! assertions on the arguments a widget hands its renderer, while [`shared`]
//! below is the headless GPU renderer the pixel oracles rasterize with.
//!
//! Each `tests/*.rs` file is its OWN test binary, so this module is compiled
//! once PER binary: every binary gets its own `shared()` renderer and thus an
//! isolated `SdfPipeline`. That isolation is deliberate - the pipeline carries
//! frame-surviving state (the shape cache, the static-background texture cache,
//! GPU buffers) ACROSS renders, so two different scenes sharing one pipeline can
//! corrupt each other's render. Keeping each scene's tests in their own binary
//! sidesteps that without touching production code; within a binary every test
//! renders the SAME scene, so the shared pipeline stays consistent.

#![allow(dead_code)]

pub mod record;

use std::sync::{Mutex, MutexGuard, OnceLock};

use iced::advanced::renderer;
use iced_wgpu::graphics::Shell;
use iced_wgpu::wgpu;
use iced_wgpu::{Engine, Renderer};

/// One shared headless renderer for the whole binary, behind a mutex.
///
/// A real app owns ONE wgpu device. Every additional device makes the SDF
/// substrate rebuild its shared shader module, layouts and pipelines, and many
/// concurrent devices can deadlock some drivers. Sharing one device and
/// serializing the GPU-touching tests behind this mutex avoids both.
/// `None` => no GPU adapter, so callers skip rather than fail.
pub fn shared() -> Option<MutexGuard<'static, Renderer>> {
    static SHARED: OnceLock<Option<Mutex<Renderer>>> = OnceLock::new();
    SHARED
        .get_or_init(|| headless_renderer().map(Mutex::new))
        .as_ref()
        .map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A headless `iced_wgpu::Renderer`, or `None` when no GPU adapter is available
/// (so the suite skips instead of failing on a GPU-less CI box).
fn headless_renderer() -> Option<Renderer> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("widget_pixel_oracle"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits {
            max_bind_groups: 2,
            ..wgpu::Limits::default()
        },
        ..Default::default()
    }))
    .ok()?;
    // No MSAA: the SDF path is analytically antialiased and we want byte-stable
    // output for determinism checks (no multisample resolve variance).
    let engine = Engine::new(
        &adapter,
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        None,
        Shell::headless(),
    );
    Some(Renderer::new(engine, renderer::Settings::default()))
}

use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Layout, layout, shell};

/// Builds `widget`'s tree and lays it out against `limits`: the diff and the
/// layout pass a runtime performs before the first draw. The returned layout is
/// the root's, at the origin.
pub fn mount<Message, Theme, Renderer>(
    widget: &mut impl Widget<Message, Theme, Renderer>,
    renderer: &Renderer,
    limits: &layout::Limits,
) -> (Tree, Layout)
where
    Renderer: iced::advanced::Renderer,
{
    let mut tree = Tree::empty();
    tree.diff(widget);
    let layout = relayout(widget, &mut tree, renderer, limits);
    (tree, layout)
}

/// Re-runs the diff and layout passes for `widget` on an existing `tree`, as a
/// runtime does for each new view, so widget state carries across frames.
pub fn relayout<Message, Theme, Renderer>(
    widget: &mut impl Widget<Message, Theme, Renderer>,
    tree: &mut Tree,
    renderer: &Renderer,
    limits: &layout::Limits,
) -> Layout
where
    Renderer: iced::advanced::Renderer,
{
    tree.diff(widget);
    widget.layout(tree, renderer, limits);
    Layout::new(tree.size)
}

/// A shell over `bus` with no window and a no-op waker, which is all
/// `Widget::update` needs outside a runtime.
pub fn headless_shell<Message>(bus: &mut shell::Bus<Message>) -> shell::Shell<'_, Message> {
    shell::Shell::new(&iced::window::Headless, shell::Waker::noop(), bus)
}
