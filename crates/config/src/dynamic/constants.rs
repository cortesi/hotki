use rhai::{Engine, Module};

use crate::{NotifyKind, Toggle};

/// Register constants used in style maps (positions, modes, weights).
pub fn register_style_constants(engine: &mut Engine) {
    let mut module = Module::new();
    insert_style_constants(&mut module);
    engine.register_global_module(module.into());
}

/// Register all DSL constants (toggles, notify kinds, plus style constants).
pub fn register_dsl_constants(engine: &mut Engine) {
    let mut module = Module::new();

    module.set_var("on", Toggle::On);
    module.set_var("off", Toggle::Off);
    module.set_var("toggle", Toggle::Toggle);

    module.set_var("ignore", NotifyKind::Ignore);
    module.set_var("info", NotifyKind::Info);
    module.set_var("warn", NotifyKind::Warn);
    module.set_var("error", NotifyKind::Error);
    module.set_var("success", NotifyKind::Success);

    insert_style_constants(&mut module);
    engine.register_global_module(module.into());
}

/// Insert the string constants shared between the DSL and theme scripts.
fn insert_style_constants(module: &mut Module) {
    module.set_var("center", "center");
    module.set_var("n", "n");
    module.set_var("ne", "ne");
    module.set_var("e", "e");
    module.set_var("se", "se");
    module.set_var("s", "s");
    module.set_var("sw", "sw");
    module.set_var("w", "w");
    module.set_var("nw", "nw");

    module.set_var("left", "left");
    module.set_var("right", "right");

    module.set_var("hud", "hud");
    module.set_var("mini", "mini");
    module.set_var("hide", "hide");

    module.set_var("thin", "thin");
    module.set_var("light", "light");
    module.set_var("regular", "regular");
    module.set_var("medium", "medium");
    module.set_var("semibold", "semibold");
    module.set_var("bold", "bold");
    module.set_var("extrabold", "extrabold");
    module.set_var("black", "black");
}
