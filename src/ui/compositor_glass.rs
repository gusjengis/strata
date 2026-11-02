// SPDX-License-Identifier: MIT

//! Compositor-drawn Liquid Glass behind the sidebar.
//!
//! GTK cannot see what lies behind its window, so Finder's sidebar glass
//! (blurred backdrop, edge bleed from the wallpaper outside the window) is
//! drawn by the compositor. Under Hyprland with the hyprglass plugin, the
//! window asks for the `apple-sidebar` material over the sidebar column via
//! `hyprglass_item_v1` (version 2 or later) and leaves that column
//! transparent. Anywhere else the sidebar keeps its opaque Finder colour.

use std::cell::RefCell;
use std::rc::Rc;

use gdk4_wayland::prelude::*;
use gtk::prelude::*;
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

mod protocol {
    #[expect(
        clippy::single_component_path_imports,
        reason = "wayland-scanner's generated code reaches the crate through super::"
    )]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("data/protocols/hyprglass-item-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("data/protocols/hyprglass-item-v1.xml");
}

use protocol::hyprglass_item_manager_v1::HyprglassItemManagerV1;
use protocol::hyprglass_item_v1::HyprglassItemV1;

/// Material the compositor draws; matches the plugin's built-in preset.
const SIDEBAR_PRESET: &str = "apple-sidebar";
/// First manager version that draws materials under a toplevel's own surface.
const MAIN_SURFACE_VERSION: u32 = 2;
const GLASS_CLASS: &str = "compositor-glass";

#[derive(Default)]
struct Registry {
    manager: Option<HyprglassItemManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Registry {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == HyprglassItemManagerV1::interface().name
            && version >= MAIN_SURFACE_VERSION
        {
            state.manager = Some(registry.bind(name, MAIN_SURFACE_VERSION, queue, ()));
        }
    }
}

impl Dispatch<HyprglassItemManagerV1, ()> for Registry {
    fn event(
        _: &mut Self,
        _: &HyprglassItemManagerV1,
        _: protocol::hyprglass_item_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<HyprglassItemV1, ()> for Registry {
    fn event(
        _: &mut Self,
        _: &HyprglassItemV1,
        _: protocol::hyprglass_item_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Surface-local logical rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// One glass hint on a window's own surface. Applies on the next commit.
struct GlassItem {
    connection: Connection,
    item: HyprglassItemV1,
    _manager: HyprglassItemManagerV1,
    _queue: EventQueue<Registry>,
    _surface: WlSurface,
}

impl GlassItem {
    fn new(window: &gtk::Window, preset: &str) -> Option<Self> {
        let surface = window
            .surface()?
            .downcast::<gdk4_wayland::WaylandSurface>()
            .ok()?;
        let display = surface
            .display()
            .downcast::<gdk4_wayland::WaylandDisplay>()
            .ok()?;
        let connection = Connection::from_backend(display.wl_display()?.backend().upgrade()?);
        let wl_surface = surface.wl_surface()?;
        let mut queue = connection.new_event_queue::<Registry>();
        let handle = queue.handle();
        let _registry = connection.display().get_registry(&handle, ());
        let mut registry = Registry::default();
        queue.roundtrip(&mut registry).ok()?;
        let manager = registry.manager?;
        let item = manager.get_item(&wl_surface, &handle, ());
        item.set_preset(preset.to_owned());
        Some(Self {
            connection,
            item,
            _manager: manager,
            _queue: queue,
            _surface: wl_surface,
        })
    }

    fn set_shape(&self, rect: Option<Rect>) {
        match rect {
            Some(r) => self
                .item
                .set_shape(r.x, r.y, r.width, r.height, 0.0, 0.0, 0.0, 0.0),
            None => self.item.unset_shape(),
        }
        let _ = self.connection.flush();
    }
}

impl Drop for GlassItem {
    fn drop(&mut self) {
        self.item.destroy();
        let _ = self.connection.flush();
    }
}

struct SidebarGlass {
    item: GlassItem,
    css: gtk::CssProvider,
    shape: Option<Rect>,
    clear_width: i32,
}

/// Puts compositor glass behind the sidebar column, which `cap` spans while
/// it carries `over-sidebar`. No effect unless the compositor supports it.
pub(in crate::ui) fn follow_sidebar(cap: &gtk::Box) {
    let state: Rc<RefCell<Option<SidebarGlass>>> = Rc::default();
    let weak_cap = cap.downgrade();
    let realized = state.clone();
    cap.connect_realize(move |cap| {
        let Some(window) = cap.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let Some(item) = GlassItem::new(&window, SIDEBAR_PRESET) else {
            tracing::debug!("compositor glass unavailable; sidebar stays opaque");
            return;
        };
        tracing::info!(preset = SIDEBAR_PRESET, "compositor glass active");
        let css = gtk::CssProvider::new();
        #[expect(
            deprecated,
            reason = "GTK 4 has no other window-local style; each sidebar has its own width"
        )]
        window
            .style_context()
            .add_provider(&css, crate::ui::theme::COMPOSITOR_GLASS_STYLE_PRIORITY);
        window.add_css_class(GLASS_CLASS);
        realized.replace(Some(SidebarGlass {
            item,
            css,
            shape: None,
            clear_width: -1,
        }));
        // Finder drops the glass while its window is inactive and the sidebar
        // turns flat; GTK repaints on the focus change, which commits the shape.
        {
            let state = realized.clone();
            let weak_cap = weak_cap.clone();
            window.connect_is_active_notify(move |window| {
                if let (Some(cap), Some(glass)) = (weak_cap.upgrade(), state.borrow_mut().as_mut())
                {
                    update(glass, &cap, window);
                }
                window.queue_draw();
            });
        }
        let Some(clock) = window.frame_clock() else {
            return;
        };
        // Layout runs after GTK allocated this frame and before it commits,
        // so the shape latches with the pixels it describes.
        let state = realized.clone();
        let weak_cap = weak_cap.clone();
        let weak_window = window.downgrade();
        clock.connect_layout(move |_| {
            let (Some(cap), Some(window)) = (weak_cap.upgrade(), weak_window.upgrade()) else {
                return;
            };
            if let Some(glass) = state.borrow_mut().as_mut() {
                update(glass, &cap, &window);
            }
        });
    });
    cap.connect_unrealize(move |cap| {
        if state.take().is_some()
            && let Some(window) = cap.root().and_downcast::<gtk::Window>()
        {
            window.remove_css_class(GLASS_CLASS);
        }
    });
}

fn update(glass: &mut SidebarGlass, cap: &gtk::Box, window: &gtk::Window) {
    let active = window.is_active();
    if active != window.has_css_class(GLASS_CLASS) {
        tracing::debug!(active, "compositor glass follows window focus");
        if active {
            window.add_css_class(GLASS_CLASS);
        } else {
            window.remove_css_class(GLASS_CLASS);
        }
    }
    let column = (active && cap.has_css_class("over-sidebar"))
        .then(|| cap.compute_bounds(window))
        .flatten()
        .filter(|bounds| bounds.width() > 0.0);
    let clear_width = column.map_or(0, |b| (b.x() + b.width()).round() as i32);
    if clear_width != glass.clear_width {
        glass.clear_width = clear_width;
        glass.css.load_from_string(&window_background(clear_width));
    }
    let (offset_x, offset_y) = window.surface_transform();
    let shape = column.map(|b| Rect {
        x: offset_x + f64::from(b.x()),
        y: offset_y,
        width: f64::from(b.width()),
        height: f64::from(window.height()),
    });
    if shape != glass.shape {
        tracing::debug!(?shape, "compositor glass shape");
        glass.shape = shape;
        glass.item.set_shape(shape);
    }
}

/// The window body stays opaque except the sidebar column, so the opaque
/// region never covers the glass.
fn window_background(clear_width: i32) -> String {
    format!(
        "window.compositor-glass {{ background-color: transparent; \
         background-image: linear-gradient(to right, transparent {clear_width}px, \
         @finder_content {clear_width}px); }}"
    )
}

#[cfg(test)]
mod tests {
    use super::window_background;

    #[test]
    fn window_background_clears_only_the_sidebar_column() {
        assert_eq!(
            window_background(156),
            "window.compositor-glass { background-color: transparent; background-image: \
             linear-gradient(to right, \
             transparent 156px, @finder_content 156px); }"
        );
    }
}
