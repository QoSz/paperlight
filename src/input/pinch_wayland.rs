//! Touchpad pinch on Wayland: `zwp_pointer_gestures_v1` bound on winit's `wl_display`.
//!
//! winit 0.30 has no Wayland pinch support, so this opens a second event queue on winit's
//! connection and forwards pinch scale changes to the UI thread.

use std::{
    sync::mpsc::{Receiver, Sender, channel},
    thread,
};

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use tracing::debug;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    backend::Backend,
    delegate_noop,
    protocol::{
        wl_pointer::WlPointer,
        wl_registry::{self, WlRegistry},
        wl_seat::{self, Capability, WlSeat},
    },
};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};

/// Binds `zwp_pointer_gestures_v1` on winit's `wl_display` and starts the detached `pinch` thread.
/// Each received value is the multiplicative zoom since the previous pinch event (>1 = fingers
/// spreading). Returns `None` (logged at debug) on X11, without the global, or on any error.
pub fn start(cc: &eframe::CreationContext<'_>, ctx: egui::Context) -> Option<Receiver<f32>> {
    let RawDisplayHandle::Wayland(handle) = cc.display_handle().ok()?.as_raw() else {
        debug!("pinch: not on Wayland");
        return None;
    };
    // SAFETY: winit's wl_display outlives this Backend: App::on_exit calls process::exit before
    // winit disconnects.
    let backend = unsafe { Backend::from_foreign_display(handle.display.as_ptr().cast()) };
    let conn = Connection::from_backend(backend);
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let (tx, rx) = channel();
    let mut state = State {
        tx,
        ctx,
        seat: None,
        pointer: None,
        gestures: None,
        pinch: None,
        last_scale: 1.0,
    };
    // The first roundtrip binds the globals; the second delivers seat capabilities (pointer, pinch).
    for _ in 0..2 {
        if let Err(e) = queue.roundtrip(&mut state) {
            debug!("pinch: roundtrip failed: {e}");
            return None;
        }
    }
    if state.pinch.is_none() {
        debug!("pinch: no pointer gestures");
        return None;
    }
    thread::Builder::new()
        .name("pinch".into())
        .spawn(move || {
            let _conn = conn;
            while queue.blocking_dispatch(&mut state).is_ok() {}
        })
        .ok()?;
    Some(rx)
}

struct State {
    tx: Sender<f32>,
    ctx: egui::Context,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    gestures: Option<ZwpPointerGesturesV1>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    last_scale: f64,
}

impl State {
    /// Creates the pinch object once both the pointer and the gestures global exist.
    fn try_create_pinch(&mut self, qh: &QueueHandle<Self>) {
        if self.pinch.is_none()
            && let (Some(pointer), Some(gestures)) = (&self.pointer, &self.gestures)
        {
            self.pinch = Some(gestures.get_pinch_gesture(pointer, qh, ()));
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(1), qh, ()));
            }
            "zwp_pointer_gestures_v1" => {
                state.gestures = Some(registry.bind(name, version.min(1), qh, ()));
                state.try_create_pinch(qh);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
            state.try_create_pinch(qh);
        }
    }
}

// Our own pointer also receives enter/motion/button/axis/frame; the plain form would panic on them.
delegate_noop!(State: ignore WlPointer);
// The interface has no events.
delegate_noop!(State: ZwpPointerGesturesV1);

impl Dispatch<ZwpPointerGesturePinchV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { .. }
            | zwp_pointer_gesture_pinch_v1::Event::End { .. } => {
                state.last_scale = 1.0;
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } if scale > 0.0 => {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "zoom factors are close to 1"
                )]
                let zoom = (scale / state.last_scale) as f32;
                let _ = state.tx.send(zoom);
                state.last_scale = scale;
                state.ctx.request_repaint();
            }
            _ => {}
        }
    }
}
