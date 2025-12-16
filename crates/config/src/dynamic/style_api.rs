use std::sync::{Arc, Mutex};

use rhai::{
    Array, Dynamic, Engine, EvalAltResult, Map, Module, NativeCallContext, serde::from_dynamic,
};

use super::{
    dsl::DynamicConfigScriptState, util::lock_unpoisoned, validation::boxed_validation_error,
};
use crate::{
    FontWeight, Mode, NotifyPos, Offset, Pos,
    raw::{Maybe, RawHud, RawNotify, RawNotifyStyle, RawStyle},
};

#[derive(Debug, Clone)]
/// Rhai-exposed style object wrapping raw style overlay data.
pub(super) struct RhaiStyle {
    /// Raw overlay data backing this style object.
    pub(super) raw: RawStyle,
}

impl RhaiStyle {
    /// Create a Rhai style object from raw overlay data.
    pub(super) fn from_raw(raw: RawStyle) -> Self {
        Self { raw }
    }

    /// Create a style from a Rhai map, validating the schema.
    fn from_map(ctx: &NativeCallContext, map: Map) -> Result<Self, Box<EvalAltResult>> {
        let dyn_map = Dynamic::from_map(map);
        let raw: RawStyle = from_dynamic(&dyn_map).map_err(|e| {
            boxed_validation_error(format!("invalid style map: {}", e), ctx.call_position())
        })?;
        Ok(Self { raw })
    }

    /// Return a new style with `other` merged over this one.
    fn merge_raw(&self, other: &RawStyle) -> Self {
        Self {
            raw: self.raw.merge(other),
        }
    }

    /// Return a new style with the given map merged over this one.
    fn merge_map(&self, ctx: &NativeCallContext, map: Map) -> Result<Self, Box<EvalAltResult>> {
        let rhs = Self::from_map(ctx, map)?;
        Ok(self.merge_raw(&rhs.raw))
    }

    /// Return a new style with a single update applied at `path`.
    fn set(
        &self,
        ctx: &NativeCallContext,
        path: &str,
        value: Dynamic,
    ) -> Result<Self, Box<EvalAltResult>> {
        let update = build_path_map(ctx, path, value)?;
        let dyn_map = Dynamic::from_map(update);
        let raw_update: RawStyle = from_dynamic(&dyn_map).map_err(|e| {
            boxed_validation_error(
                format!("invalid style update '{}': {}", path, e),
                ctx.call_position(),
            )
        })?;
        Ok(self.merge_raw(&raw_update))
    }

    /// Return the `hud` section as a Rhai map.
    fn hud_map(&self) -> Map {
        let Some(hud) = self.raw.hud.as_option() else {
            return Map::new();
        };
        raw_hud_to_map(hud)
    }

    /// Return the `notify` section as a Rhai map.
    fn notify_map(&self) -> Map {
        let Some(notify) = self.raw.notify.as_option() else {
            return Map::new();
        };
        raw_notify_to_map(notify)
    }
}

/// Register the `Style` type and its constructor/methods into the Rhai engine.
pub(super) fn register_style_type(engine: &mut Engine) {
    engine.register_type_with_name::<RhaiStyle>("Style");

    engine.register_fn(
        "Style",
        |ctx: NativeCallContext, map: Map| -> Result<RhaiStyle, Box<EvalAltResult>> {
            RhaiStyle::from_map(&ctx, map)
        },
    );

    engine.register_fn("clone", |s: RhaiStyle| s);

    engine.register_get("hud", |s: &mut RhaiStyle| s.hud_map());
    engine.register_get("notify", |s: &mut RhaiStyle| s.notify_map());

    engine.register_fn(
        "set",
        |ctx: NativeCallContext,
         s: RhaiStyle,
         path: &str,
         value: Dynamic|
         -> Result<RhaiStyle, Box<EvalAltResult>> { s.set(&ctx, path, value) },
    );

    engine.register_fn("merge", |s: RhaiStyle, other: RhaiStyle| {
        s.merge_raw(&other.raw)
    });
    engine.register_fn(
        "merge",
        |ctx: NativeCallContext, s: RhaiStyle, map: Map| -> Result<RhaiStyle, Box<EvalAltResult>> {
            s.merge_map(&ctx, map)
        },
    );
}

#[derive(Debug, Clone)]
/// Rhai-exposed theme registry namespace exported as the global `themes` variable.
pub(super) struct ThemesNamespace {
    /// Shared config script state storing the registry and active theme selection.
    state: Arc<Mutex<DynamicConfigScriptState>>,
}

/// Register the theme registry (`themes.*`) and `theme("name")` into the Rhai engine.
pub(super) fn register_theme_api(engine: &mut Engine, state: Arc<Mutex<DynamicConfigScriptState>>) {
    engine.register_type_with_name::<ThemesNamespace>("ThemesNamespace");

    engine.register_fn(
        "get",
        |ctx: NativeCallContext,
         ns: ThemesNamespace,
         name: &str|
         -> Result<RhaiStyle, Box<EvalAltResult>> {
            let guard = lock_unpoisoned(&ns.state);
            let Some(raw) = guard.themes.get(name) else {
                return Err(boxed_validation_error(
                    format!("unknown theme: {}", name),
                    ctx.call_position(),
                ));
            };
            Ok(RhaiStyle::from_raw(raw.clone()))
        },
    );

    engine.register_fn("list", |ns: ThemesNamespace| -> Array {
        let guard = lock_unpoisoned(&ns.state);
        let mut names = guard.themes.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names.into_iter().map(Dynamic::from).collect()
    });

    engine.register_fn(
        "register",
        |ns: ThemesNamespace, name: &str, style: RhaiStyle| {
            lock_unpoisoned(&ns.state)
                .themes
                .insert(name.to_string(), style.raw);
        },
    );

    engine.register_fn(
        "remove",
        |ctx: NativeCallContext,
         ns: ThemesNamespace,
         name: &str|
         -> Result<(), Box<EvalAltResult>> {
            if name == "default" {
                return Err(boxed_validation_error(
                    "themes.remove: cannot remove 'default'".to_string(),
                    ctx.call_position(),
                ));
            }

            let mut guard = lock_unpoisoned(&ns.state);
            if guard.themes.remove(name).is_none() {
                return Err(boxed_validation_error(
                    format!("themes.remove: unknown theme: {}", name),
                    ctx.call_position(),
                ));
            }

            if guard.active_theme == name {
                guard.active_theme = "default".to_string();
            }
            Ok(())
        },
    );

    register_theme_getter(engine, "default_", "default");
    register_theme_getter(engine, "charcoal", "charcoal");
    register_theme_getter(engine, "dark_blue", "dark-blue");
    register_theme_getter(engine, "solarized_dark", "solarized-dark");
    register_theme_getter(engine, "solarized_light", "solarized-light");

    {
        let state = state.clone();
        engine.register_fn(
            "theme",
            move |ctx: NativeCallContext, name: &str| -> Result<(), Box<EvalAltResult>> {
                let mut guard = lock_unpoisoned(&state);
                if !guard.themes.contains_key(name) {
                    return Err(boxed_validation_error(
                        format!("unknown theme: {}", name),
                        ctx.call_position(),
                    ));
                }
                guard.active_theme = name.to_string();
                Ok(())
            },
        );
    }

    let mut module = Module::new();
    module.set_var("themes", ThemesNamespace { state });
    engine.register_global_module(module.into());
}

/// Register a read-only convenience getter for a built-in theme name.
fn register_theme_getter(engine: &mut Engine, key: &'static str, theme_name: &'static str) {
    engine.register_get(
        key,
        move |ctx: NativeCallContext, ns: &mut ThemesNamespace| {
            let guard = lock_unpoisoned(&ns.state);
            let Some(raw) = guard.themes.get(theme_name) else {
                return Err(boxed_validation_error(
                    format!("unknown theme: {}", theme_name),
                    ctx.call_position(),
                ));
            };
            Ok(RhaiStyle::from_raw(raw.clone()))
        },
    );
}

/// Build a nested map representing `path = value` for style updates.
fn build_path_map(
    ctx: &NativeCallContext,
    path: &str,
    value: Dynamic,
) -> Result<Map, Box<EvalAltResult>> {
    let mut parts = path.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(boxed_validation_error(
            "style path cannot be empty".to_string(),
            ctx.call_position(),
        ));
    }
    if parts.iter().any(|p| p.is_empty()) {
        return Err(boxed_validation_error(
            format!("invalid style path '{}'", path),
            ctx.call_position(),
        ));
    }

    let mut current = value;
    while let Some(key) = parts.pop() {
        let mut m = Map::new();
        m.insert(key.into(), current);
        current = Dynamic::from_map(m);
    }

    Ok(current.cast())
}

/// Convert a raw HUD overlay into a Rhai map, omitting unset fields.
fn raw_hud_to_map(hud: &RawHud) -> Map {
    let mut m = Map::new();

    if let Some(v) = hud.mode.as_option() {
        m.insert("mode".into(), Dynamic::from(mode_str(*v)));
    }
    if let Some(v) = hud.pos.as_option() {
        m.insert("pos".into(), Dynamic::from(pos_str(*v)));
    }
    if let Some(v) = hud.offset.as_option() {
        m.insert("offset".into(), offset_to_map(*v));
    }
    if let Some(v) = hud.font_size.as_option() {
        m.insert("font_size".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.title_font_weight.as_option() {
        m.insert(
            "title_font_weight".into(),
            Dynamic::from(font_weight_str(*v)),
        );
    }
    if let Some(v) = hud.key_font_size.as_option() {
        m.insert("key_font_size".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.key_font_weight.as_option() {
        m.insert("key_font_weight".into(), Dynamic::from(font_weight_str(*v)));
    }
    if let Some(v) = hud.tag_font_size.as_option() {
        m.insert("tag_font_size".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.tag_font_weight.as_option() {
        m.insert("tag_font_weight".into(), Dynamic::from(font_weight_str(*v)));
    }
    if let Some(v) = hud.title_fg.as_option() {
        m.insert("title_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.bg.as_option() {
        m.insert("bg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.key_fg.as_option() {
        m.insert("key_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.key_bg.as_option() {
        m.insert("key_bg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.mod_fg.as_option() {
        m.insert("mod_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.mod_font_weight.as_option() {
        m.insert("mod_font_weight".into(), Dynamic::from(font_weight_str(*v)));
    }
    if let Some(v) = hud.mod_bg.as_option() {
        m.insert("mod_bg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.tag_fg.as_option() {
        m.insert("tag_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = hud.opacity.as_option() {
        m.insert("opacity".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.key_radius.as_option() {
        m.insert("key_radius".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.key_pad_x.as_option() {
        m.insert("key_pad_x".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.key_pad_y.as_option() {
        m.insert("key_pad_y".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.radius.as_option() {
        m.insert("radius".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = hud.tag_submenu.as_option() {
        m.insert("tag_submenu".into(), Dynamic::from(v.clone()));
    }

    m
}

/// Convert an offset struct into a Rhai map `{ x, y }`.
fn offset_to_map(offset: Offset) -> Dynamic {
    let mut m = Map::new();
    m.insert("x".into(), Dynamic::from(offset.x as f64));
    m.insert("y".into(), Dynamic::from(offset.y as f64));
    Dynamic::from_map(m)
}

/// Convert a raw notification overlay into a Rhai map, omitting unset fields.
fn raw_notify_to_map(notify: &RawNotify) -> Map {
    let mut m = Map::new();

    if let Some(v) = notify.width.as_option() {
        m.insert("width".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = notify.pos.as_option() {
        m.insert("pos".into(), Dynamic::from(notify_pos_str(*v)));
    }
    if let Some(v) = notify.opacity.as_option() {
        m.insert("opacity".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = notify.timeout.as_option() {
        m.insert("timeout".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = notify.buffer.as_option()
        && let Ok(i) = i64::try_from(*v)
    {
        m.insert("buffer".into(), Dynamic::from(i));
    }
    if let Some(v) = notify.radius.as_option() {
        m.insert("radius".into(), Dynamic::from(*v as f64));
    }

    insert_notify_style(&mut m, "info", &notify.info);
    insert_notify_style(&mut m, "warn", &notify.warn);
    insert_notify_style(&mut m, "error", &notify.error);
    insert_notify_style(&mut m, "success", &notify.success);

    m
}

/// Insert an optional per-kind notification style into a Rhai map.
fn insert_notify_style(map: &mut Map, key: &str, style: &Maybe<RawNotifyStyle>) {
    let Some(s) = style.as_option() else {
        return;
    };
    map.insert(key.into(), Dynamic::from_map(raw_notify_style_to_map(s)));
}

/// Convert a raw per-kind notification style into a Rhai map, omitting unset fields.
fn raw_notify_style_to_map(style: &RawNotifyStyle) -> Map {
    let mut m = Map::new();

    if let Some(v) = style.bg.as_option() {
        m.insert("bg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = style.title_fg.as_option() {
        m.insert("title_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = style.body_fg.as_option() {
        m.insert("body_fg".into(), Dynamic::from(v.clone()));
    }
    if let Some(v) = style.title_font_size.as_option() {
        m.insert("title_font_size".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = style.title_font_weight.as_option() {
        m.insert(
            "title_font_weight".into(),
            Dynamic::from(font_weight_str(*v)),
        );
    }
    if let Some(v) = style.body_font_size.as_option() {
        m.insert("body_font_size".into(), Dynamic::from(*v as f64));
    }
    if let Some(v) = style.body_font_weight.as_option() {
        m.insert(
            "body_font_weight".into(),
            Dynamic::from(font_weight_str(*v)),
        );
    }
    if let Some(v) = style.icon.as_option() {
        m.insert("icon".into(), Dynamic::from(v.clone()));
    }

    m
}

/// Convert HUD mode to its DSL string representation.
fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Hud => "hud",
        Mode::Hide => "hide",
        Mode::Mini => "mini",
    }
}

/// Convert HUD anchor position to its DSL string representation.
fn pos_str(pos: Pos) -> &'static str {
    match pos {
        Pos::Center => "center",
        Pos::N => "n",
        Pos::NE => "ne",
        Pos::E => "e",
        Pos::SE => "se",
        Pos::S => "s",
        Pos::SW => "sw",
        Pos::W => "w",
        Pos::NW => "nw",
    }
}

/// Convert notification stack position to its DSL string representation.
fn notify_pos_str(pos: NotifyPos) -> &'static str {
    match pos {
        NotifyPos::Left => "left",
        NotifyPos::Right => "right",
    }
}

/// Convert font weight to its DSL string representation.
fn font_weight_str(weight: FontWeight) -> &'static str {
    match weight {
        FontWeight::Thin => "thin",
        FontWeight::ExtraLight => "extralight",
        FontWeight::Light => "light",
        FontWeight::Regular => "regular",
        FontWeight::Medium => "medium",
        FontWeight::SemiBold => "semibold",
        FontWeight::Bold => "bold",
        FontWeight::ExtraBold => "extrabold",
        FontWeight::Black => "black",
    }
}
