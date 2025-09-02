use std::fmt::Write;

use eframe::egui::{self, CentralPanel, Event, Modifiers, RichText, ScrollArea, TopBottomPanel};

fn main() {
    println!("Starting Key Input Dumper...");

    let options = eframe::NativeOptions::default();

    let _ = eframe::run_native(
        "Key Input Dumper",
        options,
        Box::new(|_cc| Ok(Box::new(DumpInput::default()))),
    );
}

#[derive(Default)]
struct DumpInput {
    events: Vec<String>,
    text: String,
    show_mouse_events: bool,
}

impl eframe::App for DumpInput {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top toolbar: title, input, modifiers, clear button
        TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Key Input Dumper");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        self.events.clear();
                        self.text.clear();
                    }
                });
            });

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Type here:");
                let response = ui.text_edit_singleline(&mut self.text);
                response.request_focus();
                ui.separator();
                ui.checkbox(&mut self.show_mouse_events, "Show Mouse Events");
            });

            // Capture keyboard modifiers from context
            let mods = ctx.input(|i| i.modifiers);
            let mod_str = format!(
                "Modifiers: Cmd={}, Ctrl={}, Alt={}, Shift={}",
                mods.command, mods.ctrl, mods.alt, mods.shift
            );
            ui.label(mod_str);
        });

        // Capture and store input events
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    Event::Key {
                        key,
                        physical_key,
                        pressed,
                        repeat,
                        modifiers,
                    } => {
                        let mut s = String::new();
                        let _ = write!(
                            &mut s,
                            "{}: {}",
                            if *pressed { "KeyDown" } else { "KeyUp" },
                            key.symbol_or_name()
                        );

                        if let Some(pk) = physical_key
                            && *pk != *key
                        {
                            let _ = write!(&mut s, " | phys={}", pk.symbol_or_name());
                        }

                        if *pressed && *repeat {
                            let _ = write!(&mut s, " | repeat");
                        }

                        let _ = write!(&mut s, " | mods={}", format_modifiers(*modifiers));

                        if s.len() < 200 {
                            self.events.push(s);
                        }
                    }
                    Event::Text(t) => {
                        let escaped = escape_for_log(t);
                        let s = format!("Text: '{}' (len={})", escaped, t.chars().count());
                        if s.len() < 200 {
                            self.events.push(s);
                        }
                    }
                    Event::Paste(t) => {
                        let escaped = escape_for_log(t);
                        let s = format!("Paste: '{}' (len={})", escaped, t.chars().count());
                        if s.len() < 200 {
                            self.events.push(s);
                        }
                    }
                    Event::Copy => {
                        self.events.push("Copy".to_string());
                    }
                    Event::Cut => {
                        self.events.push("Cut".to_string());
                    }
                    other => {
                        // Optionally filter out mouse/pointer related events
                        let is_mouse_event = matches!(
                            other,
                            Event::PointerMoved(_)
                                | Event::MouseMoved(_)
                                | Event::PointerButton { .. }
                                | Event::PointerGone
                                | Event::MouseWheel { .. }
                                | Event::Zoom(_)
                        );

                        if self.show_mouse_events || !is_mouse_event {
                            let event_str = format!("{:?}", other);
                            if event_str.len() < 200 {
                                self.events.push(event_str);
                            }
                        }
                    }
                }
            }
        });

        // Main area: fills the remaining space with a scrolling log
        CentralPanel::default().show(ctx, |ui| {
            ui.label(RichText::new("Recent events").strong());
            ui.add_space(4.0);
            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for event in &self.events {
                        ui.monospace(event);
                    }
                });

            // Keep events list bounded
            if self.events.len() > 500 {
                let drain = self.events.len().saturating_sub(400);
                self.events.drain(0..drain);
            }
        });
    }
}

fn format_modifiers(mods: Modifiers) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    if mods.command {
        parts.push("Cmd");
    }
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join("+")
    }
}

fn escape_for_log(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\'' => out.push_str("\\'"),
            _ => out.push(ch),
        }
    }
    out
}
