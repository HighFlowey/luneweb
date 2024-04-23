mod enums;
mod window;

use crate::lune::util::TableBuilder;
use enums::LuaWindowEvent;
use mlua::prelude::*;
use mlua_luau_scheduler::{IntoLuaThread, LuaSchedulerExt};
use std::{cell::RefCell, collections::HashMap, time::Duration};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{EventLoop, EventLoopBuilder},
    platform::pump_events::EventLoopExtPumpEvents,
    window::{Window, WindowId},
};
use wry::WebView;

use self::window::LuaWindow;

const WINDOW_IMPL_LUA: &str = r#"
return freeze(setmetatable({
    run_script = function(...)
		return window:run_script(...)
	end,
    set_visible = function(...)
		return window:set_visible(...)
	end,
}, {
    __index = function(self, key)
        if key == "id" then
            return window.id
        end
    end,
}))
"#;

thread_local! {
    pub static WEBVIEWS: RefCell<HashMap<WindowId, WebView>> = RefCell::new(HashMap::new());
    pub static WINDOWS: RefCell<HashMap<WindowId, Window>> = RefCell::new(HashMap::new());

    // will panic if this gets accessed in another thread
    pub static EVENT_LOOP: RefCell<EventLoop<()>> = RefCell::new(EventLoopBuilder::new().build().unwrap());
}

pub struct LuaWindowId(WindowId);
impl LuaUserData for LuaWindowId {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method("__eq", |_, this, other: LuaWindowId| Ok(this.0 == other.0));
    }
}

impl<'lua> FromLua<'lua> for LuaWindowId {
    fn from_lua(value: LuaValue<'lua>, _lua: &'lua Lua) -> LuaResult<Self> {
        let userdata = value.as_userdata();

        if let Some(userdata) = userdata {
            if let Ok(this) = userdata.borrow::<Self>() {
                return Ok(Self(this.0));
            } else {
                return Err(LuaError::FromLuaConversionError {
                    from: value.type_name(),
                    to: "LuaWindowId",
                    message: None,
                });
            }
        }

        Err(LuaError::FromLuaConversionError {
            from: value.type_name(),
            to: "userdata",
            message: None,
        })
    }
}

pub fn create(lua: &Lua) -> LuaResult<LuaTable> {
    let events = TableBuilder::new(lua)?
        .with_value("Nothing", LuaWindowEvent::Nothing)?
        .with_value("Exit", LuaWindowEvent::Exit)?
        .build_readonly()?;

    TableBuilder::new(lua)?
        .with_value("events", events)?
        .with_function("new", |lua, config: LuaTable| {
            let setmetatable = lua.globals().get::<_, LuaFunction>("setmetatable")?;
            let lib = LuaWindow::new(lua, config)?;

            let table_freeze = lua
                .globals()
                .get::<_, LuaTable>("table")?
                .get::<_, LuaFunction>("freeze")?;

            let env = TableBuilder::new(lua)?
                .with_value("window", lib)?
                .with_value("setmetatable", setmetatable)?
                .with_value("freeze", table_freeze)?
                .build_readonly()?;

            lua.load(WINDOW_IMPL_LUA)
                .set_name("window")
                .set_environment(env)
                .eval::<LuaTable>()
        })?
        .with_async_function("event_loop", window_event_loop)?
        .build_readonly()
}

async fn window_event_loop<'lua>(lua: &'lua Lua, callback: LuaFunction<'lua>) -> LuaResult<()> {
    let function = lua.create_async_function(|_lua, callback: LuaFunction<'lua>| async move {
        loop {
            let mut callback_args: (Option<LuaWindowId>, LuaWindowEvent) =
                (None, LuaWindowEvent::Nothing);
            EVENT_LOOP.with(|event_loop| {
                event_loop.borrow_mut().pump_events(
                    Some(Duration::ZERO),
                    |event, elwt| match event {
                        Event::WindowEvent {
                            event: WindowEvent::CloseRequested,
                            window_id,
                            ..
                        } => {
                            let lua_window_id = LuaWindowId(window_id);
                            callback_args = (Some(lua_window_id), LuaWindowEvent::Exit);
                            elwt.exit();
                        }
                        Event::AboutToWait => {}
                        _ => (),
                    },
                );
            });

            tokio::time::sleep(Duration::ZERO).await;

            let lua_value = callback.call::<_, LuaValue>(callback_args).unwrap();

            if lua_value.is_boolean() && lua_value.as_boolean().unwrap() {
                break;
            }
        }

        Ok(())
    })?;

    let thread = function.into_lua_thread(lua)?;
    lua.push_thread_back(thread, callback)?;

    Ok(())
}
