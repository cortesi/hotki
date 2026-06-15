//! Mode-stack rendering.

use std::collections::HashSet;

use mac_keycode::Chord;
use ruau::{embed::ScriptError, session::CallOptions};
use tracing::warn;

use super::{
    Binding, BindingKind, DynamicConfig, Effect, ModeCtx, ModeFrame, RenderedState, diagnostics,
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
    cfg: &mut DynamicConfig,
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
    cfg: &mut DynamicConfig,
    frame: &ModeFrame,
    ctx: &ModeCtx,
) -> Result<(ModeView, Vec<Effect>), Error> {
    let builder =
        super::host_userdata::ModeBuilder::new_for_render(frame.style.clone(), frame.capture);
    let mut script_error = None;
    let path = cfg.path.clone();
    let sources = cfg.sources.clone();

    cfg.vm
        .step_with(
            CallOptions::new().limits(DynamicConfig::entry_limits()),
            |scope| {
                let builder_value =
                    super::host_userdata::mode_builder_userdata(scope, builder.clone())?;
                let ctx_value = super::host_userdata::mode_context_userdata(scope, ctx.clone())?;
                let render = scope.fetch_function(&frame.closure.func)?;
                let result: Result<(), ScriptError<'_>> =
                    scope.call_protected(render, (builder_value, ctx_value))?;
                if let Err(err) = result {
                    script_error = Some(diagnostics::config_script_error(
                        path.as_deref(),
                        &sources,
                        scope,
                        &err,
                    ));
                }
                Ok(())
            },
        )
        .map_err(|err| diagnostics::config_runtime_error(cfg.path.clone(), &err))?;

    if let Some(err) = script_error {
        return Err(err);
    }

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
