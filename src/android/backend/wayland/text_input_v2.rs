//! Minimal unstable text-input-v2 server for KWin's nested Wayland backend.
//!
//! KWin exposes text-input-v3 to applications inside the guest, but its Qt
//! Wayland platform client talks to the Android-facing compositor with the KDE
//! text-input-v2 protocol.  Smithay intentionally implements only v3, so this
//! small adapter owns the outer v2 endpoint and forwards activation/commits to
//! Portal's Android IME bridge.

use smithay::reexports::wayland_server::{
    backend::ClientId,
    protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use wayland_protocols_plasma::text_input::v2::server::{
    zwp_text_input_manager_v2::{self, ZwpTextInputManagerV2},
    zwp_text_input_v2::{self, ZwpTextInputV2},
};

use super::State;

#[derive(Debug)]
pub struct TextInputData {
    #[allow(dead_code)]
    seat: WlSeat,
}

pub fn register(display: &DisplayHandle) {
    display.create_global::<State, ZwpTextInputManagerV2, _>(1, ());
}

impl GlobalDispatch<ZwpTextInputManagerV2, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpTextInputManagerV2>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwpTextInputManagerV2, ()> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwpTextInputManagerV2,
        request: zwp_text_input_manager_v2::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_text_input_manager_v2::Request::GetTextInput { id, seat } => {
                let input = data_init.init(id, TextInputData { seat });
                state.text_inputs.push(input.clone());
                if let Some(surface) = state.keyboard_focus_surface.clone() {
                    state.text_input_serial = state.text_input_serial.wrapping_add(1);
                    input.enter(state.text_input_serial, &surface);
                }
            }
            zwp_text_input_manager_v2::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut Self,
        _client: ClientId,
        _resource: &ZwpTextInputManagerV2,
        _data: &(),
    ) {
    }
}

impl Dispatch<ZwpTextInputV2, TextInputData> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        input: &ZwpTextInputV2,
        request: zwp_text_input_v2::Request,
        _data: &TextInputData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_text_input_v2::Request::Enable { surface } => {
                log::info!("Nested KWin enabled Wayland text-input-v2");
                state.active_text_input = Some(input.clone());
                state.keyboard_focus_surface = Some(surface);
                crate::android::ime::set_wayland_text_input_active(true);
            }
            zwp_text_input_v2::Request::Disable { .. } => {
                log::info!("Nested KWin disabled Wayland text-input-v2");
                if state.active_text_input.as_ref() == Some(input) {
                    state.active_text_input = None;
                    crate::android::ime::set_wayland_text_input_active(false);
                }
            }
            zwp_text_input_v2::Request::ShowInputPanel => {
                log::info!("Nested KWin requested the Android input panel");
                state.active_text_input = Some(input.clone());
                crate::android::ime::request_visibility(true);
            }
            zwp_text_input_v2::Request::HideInputPanel => {
                log::info!("Nested KWin hid the Android input panel");
                crate::android::ime::request_visibility(false);
            }
            zwp_text_input_v2::Request::Destroy => {
                if state.active_text_input.as_ref() == Some(input) {
                    state.active_text_input = None;
                    crate::android::ime::set_wayland_text_input_active(false);
                }
            }
            // KWin supplies surrounding text, content purpose, cursor geometry,
            // language and update serials for the host IME. Android's editor API
            // does not need those values to provide correct committed Unicode.
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwpTextInputV2,
        _data: &TextInputData,
    ) {
        state.text_inputs.retain(Resource::is_alive);
        if state.active_text_input.as_ref() == Some(resource) {
            state.active_text_input = None;
            crate::android::ime::set_wayland_text_input_active(false);
        }
    }
}

impl State {
    pub fn update_text_input_focus(&mut self, surface: Option<WlSurface>) {
        if self.keyboard_focus_surface == surface {
            return;
        }
        let previous = std::mem::replace(&mut self.keyboard_focus_surface, surface.clone());
        self.text_input_serial = self.text_input_serial.wrapping_add(1);
        for input in self.text_inputs.iter().filter(|input| input.is_alive()) {
            if let Some(previous) = previous.as_ref() {
                input.leave(self.text_input_serial, previous);
            }
            if let Some(surface) = surface.as_ref() {
                input.enter(self.text_input_serial, surface);
            }
        }
    }

    pub fn commit_android_text(&mut self, text: &str) -> bool {
        let Some(input) = self
            .active_text_input
            .as_ref()
            .filter(|input| input.is_alive())
        else {
            return false;
        };
        if text == "\u{8}" {
            input.delete_surrounding_text(1, 0);
            input.commit_string(String::new());
        } else {
            input.commit_string(text.to_string());
        }
        true
    }
}
