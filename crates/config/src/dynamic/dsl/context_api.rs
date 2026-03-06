use rhai::{Engine, FnPtr};

use super::{
    super::{ActionCtx, ModeCtx, NavRequest},
    ModeRef, mode_id_for,
};

/// Register the shared getters exposed by both render and handler contexts.
macro_rules! register_ctx_getters {
    ($engine:expr, $ty:ty) => {
        $engine.register_get("app", |ctx: &mut $ty| ctx.app.clone());
        $engine.register_get("title", |ctx: &mut $ty| ctx.title.clone());
        $engine.register_get("pid", |ctx: &mut $ty| ctx.pid);
        $engine.register_get("hud", |ctx: &mut $ty| ctx.hud);
        $engine.register_get("depth", |ctx: &mut $ty| ctx.depth);
    };
}

/// Register `ModeCtx` and `ActionCtx` types and methods.
pub(super) fn register_context_types(engine: &mut Engine) {
    engine.register_type::<ModeCtx>();
    register_ctx_getters!(engine, ModeCtx);

    engine.register_type::<ActionCtx>();
    register_ctx_getters!(engine, ActionCtx);

    engine.register_fn("exec", |ctx: &mut ActionCtx, action: crate::Action| {
        ctx.push_effect(super::super::Effect::Exec(action));
    });
    engine.register_fn(
        "notify",
        |ctx: &mut ActionCtx, kind: crate::NotifyKind, title: &str, body: &str| {
            ctx.push_effect(super::super::Effect::Notify {
                kind,
                title: title.to_string(),
                body: body.to_string(),
            });
        },
    );
    engine.register_fn("stay", |ctx: &mut ActionCtx| {
        ctx.set_stay();
    });
    engine.register_fn("push", |ctx: &mut ActionCtx, func: FnPtr| {
        ctx.set_nav(NavRequest::Push {
            mode: ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: None,
            },
            title: None,
        });
    });
    engine.register_fn("push", |ctx: &mut ActionCtx, func: FnPtr, title: &str| {
        let title = title.to_string();
        ctx.set_nav(NavRequest::Push {
            mode: ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: Some(title.clone()),
            },
            title: Some(title),
        });
    });
    engine.register_fn("pop", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::Pop);
    });
    engine.register_fn("exit", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::Exit);
    });
    engine.register_fn("show_root", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::ShowRoot);
    });
}
