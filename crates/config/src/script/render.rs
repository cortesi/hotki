use std::{collections::HashSet, path::PathBuf, sync::OnceLock};

use mac_keycode::Chord;
use regex::Regex;
use tracing::warn;

use super::{
    Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, RenderedState,
    types::{HudRow, HudRowStyle, SourcePos},
};
use crate::{Error, NotifyKind, Style, error::excerpt_at, style};

/// Output of rendering a full stack, including user-visible warnings.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// Fully rendered state snapshot.
    pub rendered: RenderedState,
    /// Warning effects emitted during rendering.
    pub warnings: Vec<Effect>,
}

#[derive(Debug)]
/// Rendered output for one mode before stack flattening.
struct ModeView {
    /// Bindings produced by the mode renderer.
    bindings: Vec<Binding>,
    /// Optional mode-level style overlay.
    style: Option<super::StyleOverlay>,
    /// Whether this mode requested capture-all behavior.
    capture: bool,
}

/// Render the full mode stack, applying empty/orphan truncation and producing HUD rows.
pub fn render_stack(
    cfg: &DynamicConfig,
    stack: &mut Vec<ModeFrame>,
    ctx: &ModeCtx,
    base_style: &Style,
) -> Result<RenderOutput, Error> {
    let mut warnings = Vec::new();

    for depth in 0..stack.len() {
        let (view, mut local_warnings) = render_mode(cfg, &stack[depth], ctx)?;
        warnings.append(&mut local_warnings);

        let frame = &mut stack[depth];
        frame.rendered = view.bindings;
        frame.style = view.style;
        frame.capture = view.capture;

        if stack[depth].rendered.is_empty() && depth > 0 {
            stack.truncate(depth);
            break;
        }

        if depth + 1 < stack.len() {
            let Some((entered_chord, entered_mode_id)) = stack[depth + 1].entered_via.clone()
            else {
                continue;
            };
            let entry_exists = stack[depth].rendered.iter().any(|binding| {
                binding.chord == entered_chord
                    && matches!(binding.kind, BindingKind::Mode(_))
                    && binding.mode_id == Some(entered_mode_id)
            });
            if !entry_exists {
                stack.truncate(depth + 1);
                break;
            }
        }
    }

    let effective_style = compute_effective_style(base_style, stack);
    let capture = stack.last().is_some_and(|frame| frame.capture);
    let bindings = flatten_bindings(stack);
    let hud_rows = build_hud_rows(&effective_style, &bindings);

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

/// Render one mode frame and collect duplicate-chord warnings.
fn render_mode(
    cfg: &DynamicConfig,
    frame: &ModeFrame,
    ctx: &ModeCtx,
) -> Result<(ModeView, Vec<Effect>), Error> {
    cfg.reset_execution_budget();
    let builder = super::loader::ModeBuilder::new_for_render(frame.style.clone(), frame.capture);
    let builder_value = cfg
        .lua
        .create_userdata(builder.clone())
        .map_err(|err| mlua_error_to_config(cfg, &err))?;
    let ctx_value = super::loader::mode_context_userdata(&cfg.lua, ctx.clone())
        .map_err(|err| mlua_error_to_config(cfg, &err))?;

    frame
        .closure
        .func
        .call::<()>((builder_value, ctx_value))
        .map_err(|err| mlua_error_to_config(cfg, &err))?;

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

/// Keep the first binding for each chord and surface warnings for duplicates.
fn dedup_mode_bindings(cfg: &DynamicConfig, bindings: &[Binding]) -> (Vec<Binding>, Vec<Effect>) {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(bindings.len());
    let mut warnings = Vec::new();

    for binding in bindings {
        let ident = binding.chord.to_string();
        if seen.insert(ident.clone()) {
            out.push(binding.clone());
            continue;
        }

        let excerpt = binding.pos.as_ref().and_then(|pos| excerpt_for(cfg, pos));
        let loc = binding
            .pos
            .as_ref()
            .map(location_string)
            .unwrap_or_else(|| "unknown location".to_string());
        let mut body = format!("Duplicate chord '{}' ignored at {}.", ident, loc);
        if let Some(excerpt) = excerpt {
            body.push('\n');
            body.push_str(&excerpt);
        }

        warn!(target: "config::script", "{body}");
        warnings.push(Effect::Notify {
            kind: NotifyKind::Warn,
            title: "Config".to_string(),
            body,
        });
    }

    (out, warnings)
}

/// Apply every mode-level overlay in the current stack to the base style.
fn compute_effective_style(base: &Style, stack: &[ModeFrame]) -> Style {
    let overlays = stack
        .iter()
        .filter_map(|frame| frame.style.as_ref().map(|style| style.raw.clone()))
        .collect::<Vec<_>>();
    style::overlay_all_raw(base.clone(), &overlays)
}

/// Flatten local and inherited bindings into dispatch order.
fn flatten_bindings(stack: &[ModeFrame]) -> Vec<(Chord, Binding)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let Some((top, parents)) = stack.split_last() else {
        return out;
    };

    for binding in &top.rendered {
        let ident = binding.chord.to_string();
        if seen.insert(ident) {
            out.push((binding.chord.clone(), binding.clone()));
        }
    }

    for frame in parents.iter().rev() {
        for binding in frame.rendered.iter().filter(|binding| binding.flags.global) {
            let ident = binding.chord.to_string();
            if seen.insert(ident) {
                out.push((binding.chord.clone(), binding.clone()));
            }
        }
    }

    out
}

/// Build visible HUD rows from the flattened binding list.
fn build_hud_rows(base_style: &Style, bindings: &[(Chord, Binding)]) -> Vec<HudRow> {
    let mut rows = Vec::new();

    for (chord, binding) in bindings {
        let hidden =
            binding.flags.hidden || binding.style.as_ref().is_some_and(|style| style.hidden);
        if hidden {
            continue;
        }

        let style = binding.style.as_ref().and_then(|style| {
            style.overlay.as_ref().map(|overlay| {
                let resolved = style::overlay_raw(base_style.clone(), overlay);
                HudRowStyle {
                    key_fg: resolved.hud.key_fg,
                    key_bg: resolved.hud.key_bg,
                    mod_fg: resolved.hud.mod_fg,
                    mod_bg: resolved.hud.mod_bg,
                    tag_fg: resolved.hud.tag_fg,
                }
            })
        });

        rows.push(HudRow {
            chord: chord.clone(),
            desc: binding.desc.clone(),
            is_mode: matches!(binding.kind, BindingKind::Mode(_)),
            style,
        });
    }

    rows
}

/// Render the excerpt for a binding source position when its source is cached.
fn excerpt_for(cfg: &DynamicConfig, pos: &SourcePos) -> Option<String> {
    let path = pos.path.as_ref()?;
    let line = pos.line?;
    let col = pos.col.unwrap_or(1);
    let source = cfg.source_for(path)?;
    Some(excerpt_at(source.as_ref(), line, col))
}

/// Format a source position for user-facing warning messages.
fn location_string(pos: &SourcePos) -> String {
    match (&pos.path, pos.line, pos.col) {
        (Some(path), Some(line), Some(col)) => format!("{}:{}:{}", path.display(), line, col),
        (Some(path), Some(line), None) => format!("{}:{}", path.display(), line),
        (Some(path), None, _) => path.display().to_string(),
        (None, Some(line), Some(col)) => format!("line {} col {}", line, col),
        (None, Some(line), None) => format!("line {}", line),
        (None, None, _) => "unknown location".to_string(),
    }
}

/// Resolve a chord against the flattened rendered bindings.
pub fn resolve_binding<'a>(state: &'a RenderedState, chord: &Chord) -> Option<&'a Binding> {
    state.bindings.iter().find_map(|(candidate, binding)| {
        if candidate == chord {
            Some(binding)
        } else {
            None
        }
    })
}

/// Convert an `mlua` error into a `config::Error` with a best-effort source location.
pub fn mlua_error_to_config(cfg: &DynamicConfig, err: &mlua::Error) -> Error {
    let (path, line, col) =
        parse_error_location(err).unwrap_or_else(|| (cfg.path.clone(), None, None));
    let excerpt = match (&path, line, col) {
        (Some(path), Some(line), Some(col)) => cfg
            .source_for(path)
            .map(|source| excerpt_at(source.as_ref(), line, col)),
        _ => None,
    };

    match err {
        mlua::Error::SyntaxError { message, .. } => Error::Parse {
            path,
            line: line.unwrap_or(1),
            col: col.unwrap_or(1),
            message: message.clone(),
            excerpt: excerpt.unwrap_or_default(),
        },
        other => Error::Validation {
            path,
            line,
            col,
            message: other.to_string(),
            excerpt,
        },
    }
}

/// Parse a path/line/column triplet from an `mlua` error string.
pub fn parse_error_location(
    err: &mlua::Error,
) -> Option<(Option<PathBuf>, Option<usize>, Option<usize>)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let regex = RE.get_or_init(|| {
        Regex::new(r"(?m)(?P<path>/[^:\n]+|<[^:\n]+>|\[[^:\n]+\]|[^:\n]+\.luau):(?P<line>\d+)(?::(?P<col>\d+))?")
            .expect("valid Luau error regex")
    });

    let text = err.to_string();
    let caps = regex.captures(&text)?;
    let path = caps.name("path").map(|m| PathBuf::from(m.as_str()));
    let line = caps
        .name("line")
        .and_then(|m| m.as_str().parse::<usize>().ok());
    let col = caps
        .name("col")
        .and_then(|m| m.as_str().parse::<usize>().ok());
    Some((path, line, col))
}
