use crate::lune::builtins::wry::config::{EventLoopMessage, LuaDimension};
use serde::Deserialize;

// LuaWebViewMessage
#[derive(Deserialize, Debug)]
pub struct LuaWebViewMessage {
    pub __internal: bool,
    pub position: Option<LuaDimension>,
    pub mousebutton: Option<String>,
    pub keycode: Option<String>,
    pub pressed: Option<bool>,
}

impl LuaWebViewMessage {
    pub fn into_eventloop_message(self) -> Option<EventLoopMessage> {
        if let Some(position) = self.position {
            Some(EventLoopMessage::cursor_moved(position.x, position.y))
        } else if let Some(mousebutton) = self.mousebutton {
            let pressed = self.pressed.unwrap();
            Some(EventLoopMessage::mouse_button(mousebutton, pressed))
        } else if let Some(keycode) = self.keycode {
            let pressed = self.pressed.unwrap();
            Some(EventLoopMessage::keycode(keycode, pressed))
        } else {
            None
        }
    }
}
