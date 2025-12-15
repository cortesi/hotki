use std::collections::HashSet;

use mac_keycode::Chord;
use rhai::{Dynamic, EvalAltResult, Map, Position, serde::from_dynamic};
use tracing::warn;

use crate::{Error, Style, error::excerpt_at};

use super::{
    Binding, BindingKind, Effect, HudRow, HudRowStyle, ModeCtx, ModeFrame, RenderedState,
    StyleOverlay,
};

use super::{DynamicConfig, dsl::ModeBuilder};

/// Output of rendering a full stack, including user-visible warnings.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub(crate) rendered: RenderedState,
    pub(crate) warnings: Vec<Effect>,
}

#[derive(Debug)]
struct ModeView {
    bindings: Vec<Binding>,
    style: Option<StyleOverlay>,
    capture: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBindingStyle {
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    key_fg: Option<String>,
    #[serde(default)]
    key_bg: Option<String>,
    #[serde(default)]
    mod_fg: Option<String>,
    #[serde(default)]
    mod_bg: Option<String>,
    #[serde(default)]
    tag_fg: Option<String>,
}

struct ResolvedBindingStyle {
    hidden: bool,
    overlay: Option<crate::raw::RawStyle>,
}

pub fn render_stack(
    cfg: &DynamicConfig,
    stack: &mut Vec<ModeFrame>,
    ctx: &ModeCtx,
    base_style: Style,
) -> Result<RenderOutput, Error> {
    let mut warnings = Vec::new();

    // Render from root to top, truncating on empty/orphan.
    for depth in 0..stack.len() {
        let (view, mut w) = render_mode(cfg, &stack[depth], ctx)?;
        warnings.append(&mut w);

        // Update cached view on the frame.
        {
            let frame = &mut stack[depth];
            frame.rendered = view.bindings;
            frame.style = view.style;
            frame.capture = view.capture;
        }

        // Empty mode (except root) auto-pops.
        if stack[depth].rendered.is_empty() && depth > 0 {
            stack.truncate(depth);
            break;
        }

        // Orphan detection for chord-entered frames.
        if depth + 1 < stack.len() {
            let next_entered = stack[depth + 1].entered_via.clone();
            let Some((entered_chord, entered_mode_id)) = next_entered else {
                continue;
            };
            let entry_exists = stack[depth].rendered.iter().any(|b| {
                b.chord == entered_chord
                    && matches!(b.kind, BindingKind::Mode(_))
                    && b.mode_id == Some(entered_mode_id)
            });
            if !entry_exists {
                stack.truncate(depth + 1);
                break;
            }
        }
    }

    let effective_style = compute_effective_style(&base_style, stack);
    let capture = stack.last().is_some_and(|f| f.capture);

    let bindings = flatten_bindings(stack);
    let hud_rows = build_hud_rows(cfg, ctx, &effective_style, &bindings)?;

    Ok(RenderOutput {
        rendered: RenderedState {
            bindings,
            hud_rows,
            style: effective_style,
            capture,
        },
        warnings,
    })
}

fn render_mode(
    cfg: &DynamicConfig,
    frame: &ModeFrame,
    ctx: &ModeCtx,
) -> Result<(ModeView, Vec<Effect>), Error> {
    let builder = ModeBuilder::new_for_render(frame.style.clone(), frame.capture);
    let builder_for_rhai = builder.clone();

    frame
        .closure
        .func
        .call::<()>(&cfg.engine, &cfg.ast, (builder_for_rhai, ctx.clone()))
        .map_err(|err| rhai_error_to_config(cfg, &err))?;

    let (bindings, style, capture) = builder.finish();

    let (bindings, warnings) = dedup_mode_bindings(cfg, &bindings);

    Ok((
        ModeView {
            bindings,
            style,
            capture,
        },
        warnings,
    ))
}

fn dedup_mode_bindings(cfg: &DynamicConfig, bindings: &[Binding]) -> (Vec<Binding>, Vec<Effect>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(bindings.len());
    let mut warnings = Vec::new();

    for b in bindings {
        let ident = b.chord.to_string();
        if seen.insert(ident.clone()) {
            out.push(b.clone());
            continue;
        }

        let (line, col) = pos_to_line_col(b.pos);
        let excerpt = if let (Some(l), Some(c)) = (line, col) {
            Some(excerpt_at(cfg.source.as_ref(), l, c))
        } else {
            None
        };

        let loc = match (cfg.path.as_ref(), line, col) {
            (Some(path), Some(l), Some(c)) => format!("{}:{}:{}", path.display(), l, c),
            (Some(path), _, _) => format!("{}", path.display()),
            (None, Some(l), Some(c)) => format!("line {} col {}", l, c),
            (None, _, _) => "unknown location".to_string(),
        };

        let mut body = format!("Duplicate chord '{}' ignored at {}.", ident, loc);
        if let Some(ex) = excerpt {
            body.push('\n');
            body.push_str(&ex);
        }

        warn!(target: "config::dynamic", "{}", body);
        warnings.push(Effect::Notify {
            kind: crate::NotifyKind::Warn,
            title: "Config".to_string(),
            body,
        });
    }

    (out, warnings)
}

fn compute_effective_style(base: &Style, stack: &[ModeFrame]) -> Style {
    let mut overlays = Vec::new();
    for frame in stack {
        let Some(overlay) = &frame.style else {
            continue;
        };
        let Some(raw) = overlay.raw.as_ref() else {
            continue;
        };
        overlays.push(raw.clone());
    }

    base.clone().overlay_all_raw(&overlays)
}

fn flatten_bindings(stack: &[ModeFrame]) -> Vec<(Chord, Binding)> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let Some((top, parents)) = stack.split_last() else {
        return out;
    };

    for b in &top.rendered {
        let ident = b.chord.to_string();
        if seen.insert(ident) {
            out.push((b.chord.clone(), b.clone()));
        }
    }

    for frame in parents.iter().rev() {
        for b in frame.rendered.iter().filter(|b| b.flags.global) {
            let ident = b.chord.to_string();
            if seen.insert(ident) {
                out.push((b.chord.clone(), b.clone()));
            }
        }
    }

    out
}

fn build_hud_rows(
    cfg: &DynamicConfig,
    ctx: &ModeCtx,
    base_style: &Style,
    bindings: &[(Chord, Binding)],
) -> Result<Vec<HudRow>, Error> {
    let mut rows = Vec::new();

    for (ch, b) in bindings {
        let resolved = resolve_binding_style(cfg, ctx, b)?;

        if b.flags.hidden || resolved.hidden {
            continue;
        }

        let style = resolved.overlay.map(|ov| {
            let styled = base_style.clone().overlay_raw(&ov);
            HudRowStyle {
                key_fg: styled.hud.key_fg,
                key_bg: styled.hud.key_bg,
                mod_fg: styled.hud.mod_fg,
                mod_bg: styled.hud.mod_bg,
                tag_fg: styled.hud.tag_fg,
            }
        });

        rows.push(HudRow {
            chord: ch.clone(),
            desc: b.desc.clone(),
            is_mode: matches!(b.kind, BindingKind::Mode(_)),
            style,
        });
    }

    Ok(rows)
}

fn resolve_binding_style(
    cfg: &DynamicConfig,
    ctx: &ModeCtx,
    binding: &Binding,
) -> Result<ResolvedBindingStyle, Error> {
    let Some(overlay) = binding.style.as_ref() else {
        return Ok(ResolvedBindingStyle {
            hidden: false,
            overlay: None,
        });
    };

    if let Some(raw) = overlay.raw.as_ref() {
        return Ok(ResolvedBindingStyle {
            hidden: false,
            overlay: Some(raw.clone()),
        });
    }

    let Some(func) = overlay.func.as_ref() else {
        return Ok(ResolvedBindingStyle {
            hidden: false,
            overlay: None,
        });
    };

    let dyn_value = func
        .call::<Dynamic>(&cfg.engine, &cfg.ast, (ctx.clone(),))
        .map_err(|err| rhai_error_to_config(cfg, &err))?;

    if !dyn_value.is::<Map>() {
        return Err(Error::Validation {
            path: cfg.path.clone(),
            line: None,
            col: None,
            message: format!(
                "binding style closure must return a map, got {}",
                dyn_value.type_name()
            ),
            excerpt: None,
        });
    }

    let map: Map = dyn_value.cast();
    let dyn_map = Dynamic::from_map(map);
    let style: RawBindingStyle = from_dynamic(&dyn_map).map_err(|e| Error::Validation {
        path: cfg.path.clone(),
        line: None,
        col: None,
        message: format!("invalid binding style map: {}", e),
        excerpt: None,
    })?;

    let hidden = style.hidden.unwrap_or(false);

    let mut hud = crate::raw::RawHud::default();
    if let Some(v) = style.key_fg {
        hud.key_fg = crate::raw::Maybe::Value(v);
    }
    if let Some(v) = style.key_bg {
        hud.key_bg = crate::raw::Maybe::Value(v);
    }
    if let Some(v) = style.mod_fg {
        hud.mod_fg = crate::raw::Maybe::Value(v);
    }
    if let Some(v) = style.mod_bg {
        hud.mod_bg = crate::raw::Maybe::Value(v);
    }
    if let Some(v) = style.tag_fg {
        hud.tag_fg = crate::raw::Maybe::Value(v);
    }

    let overlay = if hud.key_fg.as_option().is_some()
        || hud.key_bg.as_option().is_some()
        || hud.mod_fg.as_option().is_some()
        || hud.mod_bg.as_option().is_some()
        || hud.tag_fg.as_option().is_some()
    {
        Some(crate::raw::RawStyle {
            hud: crate::raw::Maybe::Value(hud),
            notify: crate::raw::Maybe::Unit(()),
        })
    } else {
        None
    };

    Ok(ResolvedBindingStyle { hidden, overlay })
}

pub fn resolve_binding<'a>(state: &'a RenderedState, chord: &Chord) -> Option<&'a Binding> {
    state
        .bindings
        .iter()
        .find_map(|(ch, b)| if ch == chord { Some(b) } else { None })
}

fn pos_to_line_col(pos: Position) -> (Option<usize>, Option<usize>) {
    let line = pos.line().map(|l| l.max(1));
    let col = pos.position().map(|c| c.max(1));
    (line, col)
}

pub(crate) fn rhai_error_to_config(cfg: &DynamicConfig, err: &EvalAltResult) -> Error {
    let (line, col) = pos_to_line_col(err.position());
    let excerpt = match (line, col) {
        (Some(l), Some(c)) => Some(excerpt_at(cfg.source.as_ref(), l, c)),
        _ => None,
    };
    Error::Validation {
        path: cfg.path.clone(),
        line,
        col,
        message: err.to_string(),
        excerpt,
    }
}
