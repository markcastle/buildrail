//! The Strip page: how the strip looks and where it sits. Every control
//! applies as it changes (the controller saves the preferences
//! atomically), and the Saved mark confirms it. Controls follow the
//! recipe: one of a few is a segmented control, on or off is a switch, a
//! physical display is a picture.

use buildrail_profile::{DevicePrefs, MonitorChoice, Placement, Theme};
use eframe::egui::{self, Align, Layout, Ui};

use super::forms::sentence;
use super::SettingsWindow;
use crate::app::AppController;
use crate::ui::theme::Tokens;
use crate::ui::widgets::{
    self, display_picker, error_line, group, page_header, saved_mark, segmented, toggle,
    DisplayTile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacementChoice {
    Docked,
    Floating,
}

impl SettingsWindow {
    /// Draws the page; returns `true` when placement, display or scale
    /// changed (the strip must be placed again).
    pub(super) fn strip_page(
        &mut self,
        ui: &mut Ui,
        controller: &mut AppController,
        device: &super::DeviceInfo,
        tokens: &Tokens,
    ) -> bool {
        let mut prefs: DevicePrefs = controller.prefs().clone();
        let before = prefs.clone();
        let saved = self.saved_visible(ui);
        ui.horizontal_top(|ui| {
            ui.set_width(ui.available_width().min(widgets::group::MAX_WIDTH));
            page_header(
                ui,
                tokens,
                "Strip",
                "How the strip looks and where it sits. Changes apply as you make them.",
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            ui.set_width(ui.available_width().min(widgets::group::MAX_WIDTH));
            ui.set_height(0.0);
            saved_mark(ui, tokens, saved);
        });

        group(ui, tokens, |g| {
            g.row(
                "Theme",
                Some("System follows your computer's light or dark setting."),
                |ui| {
                    segmented(
                        ui,
                        tokens,
                        "Theme",
                        &mut prefs.theme,
                        &[
                            (Theme::System, "System"),
                            (Theme::Light, "Light"),
                            (Theme::Dark, "Dark"),
                        ],
                    );
                },
            );
            let docked_help = "Docked, the strip sits along the bottom edge and windows make room for it. Floating, you can drag it anywhere by its caption.";
            let help = device.placement_note.as_deref().unwrap_or(docked_help);
            g.row("Placement", Some(help), |ui| {
                let mut choice = match prefs.placement {
                    Placement::DockedBottom => PlacementChoice::Docked,
                    Placement::Floating { .. } => PlacementChoice::Floating,
                };
                let changed = segmented(
                    ui,
                    tokens,
                    "Placement",
                    &mut choice,
                    &[
                        (PlacementChoice::Docked, "Docked"),
                        (PlacementChoice::Floating, "Floating"),
                    ],
                )
                .changed();
                if changed {
                    prefs.placement = match choice {
                        PlacementChoice::Docked => Placement::DockedBottom,
                        PlacementChoice::Floating => Placement::Floating { position: None },
                    };
                }
            });
            if device.monitors.len() > 1 {
                g.stack(
                    "Display",
                    Some("The strip sits on the display you pick."),
                    |ui| {
                        let tiles: Vec<DisplayTile> = device
                            .monitors
                            .iter()
                            .map(|m| DisplayTile {
                                name: if m.primary {
                                    format!("{} (main)", m.name)
                                } else {
                                    m.name.clone()
                                },
                                width: m.width,
                                height: m.height,
                            })
                            .collect();
                        let chosen = match &prefs.monitor {
                            Some(choice) => device.monitors.iter().position(|m| m.id == choice.id),
                            None => device.monitors.iter().position(|m| m.primary),
                        };
                        if let Some(index) = display_picker(ui, tokens, &tiles, chosen) {
                            let monitor = &device.monitors[index];
                            prefs.monitor = (!monitor.primary).then(|| MonitorChoice {
                                id: monitor.id.clone(),
                                name: Some(monitor.name.clone()),
                            });
                        }
                    },
                );
            }
            let mut custom = prefs.scale.is_some();
            g.row(
                "Custom scale",
                Some("Off, the strip follows the display's own scaling."),
                |ui| {
                    if toggle(ui, tokens, &mut custom, "Custom scale").changed() {
                        prefs.scale = custom.then_some(1.0);
                    }
                },
            );
            if let Some(scale) = prefs.scale.as_mut() {
                g.row("Scale", Some("How big the strip and its text are."), |ui| {
                    let mut value = *scale as f32;
                    let slider = egui::Slider::new(&mut value, 0.5..=4.0)
                        .step_by(0.25)
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0));
                    if ui.add(slider).changed() {
                        *scale = f64::from(value);
                    }
                });
            }
            g.row(
                "Animation",
                Some("Turns the in-progress mark while a build runs. Off, nothing on the strip moves."),
                |ui| {
                    toggle(ui, tokens, &mut prefs.animation, "Animation");
                },
            );
            g.label("Startup");
            g.row(
                "Start at login",
                Some("BuildRail opens when you sign in to your computer, so the strip is there when you are."),
                |ui| {
                    toggle(ui, tokens, &mut prefs.launch_at_startup, "Start at login");
                },
            );
        });
        if device.monitors.len() <= 1 {
            sentence(
                ui,
                tokens,
                "With more than one display, you can choose here which one the strip sits on.",
            );
        }

        let mut placement_changed = false;
        if prefs != before {
            placement_changed = prefs.placement != before.placement
                || prefs.monitor != before.monitor
                || prefs.scale != before.scale;
            match controller.update_prefs(|p| *p = prefs) {
                Ok(()) => {
                    self.device_message = None;
                    self.mark_saved(ui);
                }
                Err(error) => {
                    self.device_message = Some(error);
                    placement_changed = false;
                }
            }
        }
        if let Some(message) = &self.device_message {
            error_line(
                ui,
                tokens,
                &format!("BuildRail could not save this change: {message}. Try again."),
            );
        }
        placement_changed
    }
}
