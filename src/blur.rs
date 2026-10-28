//! Native blur for modern KWin (ext-background-effect-v1), with winit's legacy fallback.
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::sync::Arc;
use wayland_client::{
    backend::{Backend, ObjectId},
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
#[derive(Default)]
struct State {
    supports_blur: bool,
}
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            state.supports_blur = flags
                .into_result()
                .is_ok_and(|v| v.contains(ext_background_effect_manager_v1::Capability::Blur));
        }
    }
}
delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore ExtBackgroundEffectSurfaceV1);
delegate_noop!(State: ignore XdgActivationV1);
pub struct Blur {
    effect: Option<ExtBackgroundEffectSurfaceV1>,
    manager: Option<ExtBackgroundEffectManagerV1>,
    activation: Option<XdgActivationV1>,
    surface: WlSurface,
    queue: EventQueue<State>,
    state: State,
    connection: Connection,
    // Keeps both the foreign display and surface alive until our protocol objects are destroyed.
    _window: Arc<winit::window::Window>,
}
impl Blur {
    pub fn install(window: Arc<winit::window::Window>) -> Option<Self> {
        let result = Self::modern(window.clone());
        if result.as_ref().is_none_or(|b| b.effect.is_none()) {
            window.set_blur(true);
        }
        result
    }
    fn modern(window: Arc<winit::window::Window>) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(surface) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit owns these valid Wayland pointers. The stored Arc retains the window,
        // and the guest backend does not own or disconnect winit's display. No ownership of
        // winit's surface is taken; it is only referenced as a protocol request argument.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.surface.as_ptr().cast()) }
                .ok()?;
        let connection = Connection::from_backend(backend);
        let surface = WlSurface::from_id(&connection, id).ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&connection).ok()?;
        let qh = queue.handle();
        let mut manager: Option<ExtBackgroundEffectManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        let activation: Option<XdgActivationV1> = globals.bind(&qh, 1..=1, ()).ok();
        if manager.is_none() && activation.is_none() {
            return None;
        }
        let mut state = State::default();
        queue.roundtrip(&mut state).ok()?;
        if !state.supports_blur {
            if let Some(manager) = manager.take() {
                manager.destroy();
            }
        }
        let effect = manager.as_ref().map(|manager| {
            let effect = manager.get_background_effect(&surface, &qh, ());
            if let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&qh, 1..=1, ()) {
                let region = compositor.create_region(&qh, ());
                // The compositor clips the region, including after window resizes.
                region.add(0, 0, 1_000_000, 1_000_000);
                effect.set_blur_region(Some(&region));
                region.destroy();
            }
            effect
        });
        // Applied by winit's next wl_surface.commit. Do not take over the surface's render lifecycle.
        connection.flush().ok()?;
        Some(Self {
            effect,
            manager,
            activation,
            surface,
            queue,
            state,
            connection,
            _window: window,
        })
    }
    pub fn dispatch(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let _ = self.connection.flush();
    }
    pub fn activate(&mut self, token: &str) {
        if let Some(activation) = &self.activation {
            if !token.is_empty() {
                activation.activate(token.to_owned(), &self.surface);
                let _ = self.connection.flush();
            }
        }
    }
}
impl Drop for Blur {
    fn drop(&mut self) {
        if let Some(effect) = &self.effect {
            effect.destroy();
        }
        if let Some(manager) = &self.manager {
            manager.destroy();
        }
        if let Some(activation) = &self.activation {
            activation.destroy();
        }
        let _ = self.connection.flush();
    }
}
