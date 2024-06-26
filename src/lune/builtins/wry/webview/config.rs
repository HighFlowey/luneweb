use crate::lune::{
    builtins::serde::encode_decode::{EncodeDecodeConfig, EncodeDecodeFormat},
    util::{connection::create_connection_handler, http::lua_table_to_headers},
};
use http::HeaderMap;
use mlua::prelude::*;
use mlua_luau_scheduler::{LuaSchedulerExt, LuaSpawnExt};
use std::{collections::HashMap, rc::Weak, time::Duration};
use wry::WebView;

// LuaWebView
pub struct LuaWebView {
    pub webview: WebView,
    pub ipc_sender: tokio::sync::watch::Sender<String>,
}

impl LuaUserData for LuaWebView {
    fn add_methods<'lua, M: LuaUserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method(
            "evaluate_noresult",
            |_lua: &Lua, this: &Self, script: String| {
                this.webview.evaluate_script(script.as_str()).into_lua_err()
            },
        );

        methods.add_method(
            "evaluate_callback",
            |lua: &Lua, this: &Self, (script, callback): (String, LuaFunction)| {
                let (result_tx, mut result_rx) = tokio::sync::watch::channel("null".to_string());

                this.webview
                    .evaluate_script_with_callback(script.as_str(), move |res| {
                        result_tx.send(res.clone()).into_lua_err().unwrap();
                    })
                    .into_lua_err()?;

                let inner_lua = lua
                    .app_data_ref::<Weak<Lua>>()
                    .expect("Missing weak lua ref")
                    .upgrade()
                    .expect("Lua was dropped unexpectedly");

                let callback_key = lua.create_registry_value(callback)?;

                lua.spawn_local(async move {
                    if result_rx.changed().await.is_ok() {
                        let inner_callback = inner_lua
                            .registry_value::<LuaFunction>(&callback_key)
                            .unwrap();

                        let borrowed = result_rx.borrow_and_update();
                        let config = EncodeDecodeConfig::from(EncodeDecodeFormat::Json);
                        let result = config
                            .deserialize_from_string(&inner_lua, borrowed.as_str().into())
                            .unwrap();

                        let thread = inner_lua.create_thread(inner_callback).unwrap();
                        inner_lua.push_thread_back(thread, result).unwrap();
                    }
                });

                Ok(())
            },
        );

        methods.add_async_method(
            "evaluate",
            |lua: &Lua, this: &Self, script: String| async move {
                let (result_tx, mut result_rx) = tokio::sync::watch::channel("null".to_string());

                this.webview
                    .evaluate_script_with_callback(script.as_str(), move |res| {
                        result_tx.send(res.clone()).into_lua_err().unwrap();
                    })
                    .into_lua_err()?;

                if result_rx.changed().await.is_ok() {
                    let borrowed = result_rx.borrow_and_update();
                    let config = EncodeDecodeConfig::from(EncodeDecodeFormat::Json);
                    config.deserialize_from_string(lua, borrowed.as_str().into())
                } else {
                    Ok(LuaValue::Nil)
                }
            },
        );

        methods.add_async_method(
            "ipc_handler",
            |lua: &Lua, this: &Self, callback: LuaFunction<'_>| async move {
                let callback_key = lua.create_registry_value(callback)?;

                let inner_lua = lua
                    .app_data_ref::<Weak<Lua>>()
                    .expect("Missing weak lua ref")
                    .upgrade()
                    .expect("Lua was dropped unexpectedly");

                let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
                let listener = this.ipc_sender.subscribe();

                lua.spawn_local(async move {
                    let mut inner_listener = listener.clone();

                    let inner_callback = inner_lua
                        .registry_value::<LuaFunction>(&callback_key)
                        .unwrap();

                    loop {
                        tokio::select! {
                            _ = inner_listener.changed() => {

                            },
                            res = shutdown_rx.changed() => {
                                if res.is_ok() {
                                    break;
                                }
                            }
                        }

                        let message = inner_listener.borrow_and_update().clone();
                        let thread = inner_lua
                            .create_thread(inner_callback.clone())
                            .into_lua_err()
                            .unwrap();
                        let config = EncodeDecodeConfig::from(EncodeDecodeFormat::Json);
                        let res = config
                            .deserialize_from_string(&inner_lua, message.into())
                            .unwrap();
                        inner_lua
                            .push_thread_back(thread, res)
                            .into_lua_err()
                            .unwrap();

                        tokio::time::sleep(Duration::ZERO).await;
                    }
                });

                create_connection_handler(lua, shutdown_tx)
            },
        );

        methods.add_method("load_url", |_lua: &Lua, this: &Self, url: String| {
            this.webview.load_url(url.as_str()).into_lua_err()
        });

        methods.add_method(
            "load_url_with_headers",
            |lua: &Lua, this: &Self, (url, headers): (String, LuaTable)| {
                this.webview
                    .load_url_with_headers(url.as_str(), lua_table_to_headers(Some(headers), lua)?)
                    .into_lua_err()
            },
        );
    }
}

// LuaWebViewConfig
pub struct LuaWebViewConfig {
    pub with_devtools: bool,
    pub init_script: Option<String>,
    pub html: Option<String>,
    pub url: Option<String>,
    pub headers: HeaderMap,
    pub custom_protocols: HashMap<String, LuaRegistryKey>,
}

impl<'lua> FromLua<'lua> for LuaWebViewConfig {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> LuaResult<Self> {
        if let Some(config) = value.as_table() {
            Ok(Self {
                with_devtools: config.get("with_devtools").unwrap_or(false),
                init_script: config.get("init_script").ok(),
                html: config.get("html").ok(),
                url: config.get("url").ok(),
                headers: lua_table_to_headers(config.get("headers").ok(), lua)?,
                custom_protocols: config.get("custom_protocols").unwrap_or_default(),
            })
        } else {
            Err(LuaError::FromLuaConversionError {
                from: value.type_name(),
                to: "table",
                message: None,
            })
        }
    }
}

// LuaWebViewScript
pub struct LuaWebViewScript {
    raw: String,
}

impl LuaWebViewScript {
    pub fn new() -> Self {
        Self { raw: String::new() }
    }

    pub fn read(self) -> Box<str> {
        self.raw.as_str().into()
    }

    pub fn write(&mut self, string: &str) {
        self.raw += string;
        self.raw.push(';');
    }

    pub fn extract_from_option<T>(&mut self, option: Option<T>)
    where
        T: AsRef<str> + std::default::Default,
    {
        let binding = option.unwrap_or_default();
        let string = binding.as_ref();
        self.raw += string;
        self.raw.push(';');
    }
}
