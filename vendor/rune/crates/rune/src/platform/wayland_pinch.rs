//! Touchpad pinch on Wayland. winit 0.30 only reports pinch gestures on macOS and iOS, so on
//! Wayland a pinch never reaches the app. This binds `zwp_pointer_gestures_v1` on winit's own
//! display connection, in a private queue read by one thread (the same arrangement as
//! `activation`), and turns each pinch over the window into a `WakeEvent::Pinch`, which the
//! window handles like winit's `PinchGesture`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

use crate::GesturePhase;
use crate::window::WakeEvent;

const NOT_TRIED: u8 = 0;
const LISTENING: u8 = 1;
/// The compositor has no `zwp_pointer_gestures_v1`.
const UNSUPPORTED: u8 = 2;
const FAILED: u8 = 3;

static STATUS: AtomicU8 = AtomicU8::new(NOT_TRIED);

/// True once a pinch gesture is bound: a pinch over a window reaches its `on_magnify`.
pub fn active() -> bool {
    STATUS.load(Ordering::Relaxed) == LISTENING
}

/// Why the pinch is not active, for an app that wants to say so.
pub fn why_not() -> Option<&'static str> {
    match STATUS.load(Ordering::Relaxed) {
        UNSUPPORTED => Some("il compositor non offre i gesti del touchpad"),
        FAILED => Some("non sono riuscito ad ascoltare i gesti del touchpad"),
        _ => None,
    }
}

struct State {
    proxy: EventLoopProxy<WakeEvent>,
    slot: usize,
    /// The window's `wl_surface`: a pinch is ours only when it begins over it.
    surface: ObjectId,
    gestures: ZwpPointerGesturesV1,
    pointer: Option<WlPointer>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    /// The pinch in progress began over the window.
    ours: bool,
    /// The scale of the last update; a pinch reports the scale since it began.
    scale: f64,
    /// `RUNE_PINCH_DEBUG` set: say on stderr what arrives.
    debug: bool,
}

impl State {
    fn send(&self, delta: f64, phase: GesturePhase) {
        let _ = self.proxy.send_event(WakeEvent::Pinch(self.slot, delta as f32, phase));
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else { return };
        if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            state.pinch = Some(state.gestures.get_pinch_gesture(&pointer, qh, ()));
            state.pointer = Some(pointer);
            STATUS.store(LISTENING, Ordering::Relaxed);
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(_: &mut Self, _: &WlPointer, _: <WlPointer as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpPointerGesturesV1, ()> for State {
    fn event(_: &mut Self, _: &ZwpPointerGesturesV1, _: <ZwpPointerGesturesV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for State {
    fn event(state: &mut Self, _: &ZwpPointerGesturePinchV1, event: zwp_pointer_gesture_pinch_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_pointer_gesture_pinch_v1::Event;
        match event {
            Event::Begin { surface, fingers, .. } => {
                // The protocol number of an object is the same whichever backend wraps it.
                state.ours = surface.id().protocol_id() == state.surface.protocol_id();
                state.scale = 1.0;
                if state.debug {
                    eprintln!("rune pinch: begin, {fingers} fingers, over {}", if state.ours { "this window" } else { "another surface" });
                }
                if state.ours {
                    state.send(0.0, GesturePhase::Began);
                }
            }
            Event::Update { scale, .. } if state.ours && scale > 0.0 => {
                let delta = scale / state.scale - 1.0;
                state.scale = scale;
                if state.debug {
                    eprintln!("rune pinch: scale {scale:.3} (step {delta:+.3})");
                }
                state.send(delta, GesturePhase::Moved);
            }
            Event::End { .. } if state.ours => {
                state.ours = false;
                if state.debug {
                    eprintln!("rune pinch: end");
                }
                state.send(0.0, GesturePhase::Ended);
            }
            _ => {}
        }
    }
}

/// One window's pinch listener; dropping it stops the thread and releases the gesture.
pub(crate) struct PinchListener {
    stop: Arc<AtomicBool>,
}

impl PinchListener {
    /// None off Wayland or when the compositor lacks `zwp_pointer_gestures_v1`.
    pub fn new(window: &Window, proxy: EventLoopProxy<WakeEvent>, slot: usize) -> Option<PinchListener> {
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) = (window.display_handle().ok()?.as_raw(), window.window_handle().ok()?.as_raw()) else {
            return None;
        };
        // SAFETY: winit owns this `wl_display` and keeps it alive for the event loop, which
        // outlives every window; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = match registry_queue_init::<State>(&conn) {
            Ok(g) => g,
            Err(_) => {
                STATUS.store(FAILED, Ordering::Relaxed);
                return None;
            }
        };
        let qh = queue.handle();
        let Ok(gestures) = globals.bind::<ZwpPointerGesturesV1, _, _>(&qh, 1..=3, ()) else {
            STATUS.store(UNSUPPORTED, Ordering::Relaxed);
            return None;
        };
        let Ok(seat) = globals.bind::<WlSeat, _, _>(&qh, 1..=7, ()) else {
            STATUS.store(FAILED, Ordering::Relaxed);
            return None;
        };
        // SAFETY: the pointer is the live `wl_surface` proxy winit created for this window; only
        // its identity is compared, and the listener is dropped before the window.
        let surface = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }.ok()?;
        let debug = std::env::var_os("RUNE_PINCH_DEBUG").is_some();
        let mut state = State { proxy, slot, surface, gestures, pointer: None, pinch: None, ours: false, scale: 1.0, debug };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("rune-pinch".into())
            .spawn(move || {
                let _keep = (conn, seat);
                // The pointer moving over the window wakes this, so a stop is seen within moments.
                while !flag.load(Ordering::Relaxed) {
                    if queue.blocking_dispatch(&mut state).is_err() {
                        STATUS.store(FAILED, Ordering::Relaxed);
                        break;
                    }
                }
                if let Some(p) = state.pinch.take() {
                    p.destroy();
                }
                if let Some(p) = state.pointer.take() {
                    p.release();
                }
                let _ = queue.flush();
            })
            .ok()?;
        Some(PinchListener { stop })
    }
}

impl Drop for PinchListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
