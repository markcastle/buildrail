//! The Settings window, built on Task Rail's settings recipe: a
//! `SettingsNav` down the left (Builds, Connections, Strip, Share, About,
//! then Done and Quit BuildRail), and on the right one page at a time, each
//! a `PageHeader` over `SettingsGroup`s.
//!
//! This module only *draws* the settings state machines and turns clicks
//! into their inputs; all decisions are in [`crate::settings`]. Each frame:
//!
//! 1. drawing collects inputs (it never mutates a machine mid-draw),
//! 2. inputs are applied to the machines, which return [`Command`]s,
//! 3. commands go to the controller; their results come back through
//!    [`SettingsWindow::handle_events`].
//!
//! Preferences save themselves as they change (the controller writes them
//! atomically) and a quiet "Saved" mark confirms it. Only actions keep verb
//! buttons: Add build, Connect, Pin build, Export, Import.
//!
//! Pages live in their own files: [`builds`] (pinned builds and the
//! Add-build wizard in [`wizard`]), [`connections`], [`strip`], [`share`]
//! and [`about`]; [`forms`] holds what they share (the token form, list
//! loading, descriptions).

mod about;
mod builds;
mod connections;
mod forms;
mod share;
mod strip;
mod wizard;

use buildrail_core::domain::WatchId;
use buildrail_profile::ImportPreview;
use eframe::egui::{self, pos2, Margin, Rect, Stroke, Ui, UiBuilder, Vec2};

use super::theme::{space, type_style, Tokens};
use super::widgets::{self, nav_item, NavItem};
use crate::app::AppController;
use crate::settings::import::{ImportFlow, ImportInput};
use crate::settings::manage::{ManageInput, ManageList};
use crate::settings::wizard::{AddBuildWizard, Step, WizardInput};
use crate::settings::{Command, Event, SettingsContext};

/// A Settings page, named by what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Pinned builds: rename, reorder, remove, reconnect; and Add build.
    Builds,
    /// Connections on this device.
    Connections,
    /// How the strip looks and where it sits: theme, placement, display,
    /// scale, start at login.
    Strip,
    /// Import and export.
    Share,
    /// Version, licence and where things are kept.
    About,
}

impl Page {
    const ALL: [(Page, &'static str); 5] = [
        (Page::Builds, "Builds"),
        (Page::Connections, "Connections"),
        (Page::Strip, "Strip"),
        (Page::Share, "Share"),
        (Page::About, "About"),
    ];
}

/// One connected display, for the display picker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorInfo {
    /// The platform's identifier (what the preference stores).
    pub id: String,
    /// A name for people.
    pub name: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// The OS's main display.
    pub primary: bool,
}

/// Facts about the window the Strip page shows.
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    /// Connected displays.
    pub monitors: Vec<MonitorInfo>,
    /// How the strip is placed right now, in words.
    pub placement_note: Option<String>,
}

/// What the window asks of its host after a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsOutcome {
    /// Placement, monitor or scale changed: re-place the strip.
    pub placement_changed: bool,
    /// The person pressed Done: close Settings.
    pub close: bool,
    /// The person chose Quit BuildRail.
    pub quit: bool,
}

/// State of the Settings window. See the module docs.
#[derive(Debug)]
pub struct SettingsWindow {
    /// The page shown.
    pub page: Page,
    /// `true` while the Add-build wizard is shown (on the Builds page).
    adding: bool,
    wizard: AddBuildWizard,
    manage: ManageList,
    import: ImportFlow,
    filter: String,
    rename_buffer: String,
    import_path: String,
    export_path: String,
    share_message: Option<(widgets::Tone, String)>,
    device_message: Option<String>,
    about_message: Option<String>,
    /// When a preference last landed (egui time), for the Saved mark.
    saved_at: Option<f64>,
    /// The build opened from its details panel: outlined on the Builds
    /// page until the person does something there.
    highlight: Option<WatchId>,
    /// Scroll the highlighted build into view on the next frame.
    scroll_to_highlight: bool,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// An input for one of the machines, collected while drawing.
enum Input {
    Wizard(WizardInput),
    Manage(ManageInput),
    Import(ImportInput),
}

impl SettingsWindow {
    /// A window on the Builds page.
    pub fn new() -> Self {
        Self {
            page: Page::Builds,
            adding: false,
            wizard: AddBuildWizard::new(),
            manage: ManageList::new(),
            import: ImportFlow::new(),
            filter: String::new(),
            rename_buffer: String::new(),
            import_path: String::new(),
            export_path: String::new(),
            share_message: None,
            device_message: None,
            about_message: None,
            saved_at: None,
            highlight: None,
            scroll_to_highlight: false,
        }
    }

    /// The wizard (for tests and the host).
    pub fn wizard(&self) -> &AddBuildWizard {
        &self.wizard
    }

    /// The import flow (for tests and the host).
    pub fn import(&self) -> &ImportFlow {
        &self.import
    }

    /// The import preview, once a profile has been read.
    pub fn import_preview(&self) -> Option<&ImportPreview> {
        self.import.preview()
    }

    /// `true` while the Add-build wizard is shown.
    pub fn is_adding(&self) -> bool {
        self.page == Page::Builds && self.adding
    }

    /// Shows the Add-build wizard, starting over if the last one finished.
    pub fn open_add_build(&mut self) {
        if self.wizard.step() == Step::Pinned {
            self.wizard = AddBuildWizard::new();
        }
        self.page = Page::Builds;
        self.adding = true;
    }

    /// Shows the Builds page with `watch` outlined and scrolled into view
    /// (the details panel's Edit in Settings).
    pub fn open_build(&mut self, watch: &WatchId) {
        self.page = Page::Builds;
        self.adding = false;
        self.highlight = Some(watch.clone());
        self.scroll_to_highlight = true;
    }

    /// The build outlined on the Builds page, if any.
    pub fn highlighted(&self) -> Option<&WatchId> {
        self.highlight.as_ref()
    }

    /// Shows the Builds page with the reconnect form open for `watch`.
    pub fn open_reconnect(&mut self, watch: &WatchId, controller: &AppController) {
        self.page = Page::Builds;
        self.adding = false;
        let ctx = context(controller);
        let _ = self
            .manage
            .input(ManageInput::StartReconnect(watch.clone()), ctx);
    }

    /// Routes controller events to every machine; returns follow-up
    /// commands (e.g. the wizard loading repositories after a connect).
    pub fn handle_events(&mut self, events: &[Event]) -> Vec<Command> {
        let mut commands = Vec::new();
        for event in events {
            commands.extend(self.wizard.on_event(event));
            self.manage.on_event(event);
            self.import.on_event(event);
        }
        commands
    }

    /// Draws the window into `ui` (all of it) and executes the resulting
    /// commands.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        controller: &mut AppController,
        device: &DeviceInfo,
        tokens: &Tokens,
    ) -> SettingsOutcome {
        let mut inputs = Vec::new();
        let mut outcome = SettingsOutcome::default();
        let size = ui.available_size().max(Vec2::new(600.0, 400.0));
        let (full, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().rect_filled(full, 0.0, tokens.bg);

        let nav_rect = Rect::from_min_size(full.min, Vec2::new(NAV_WIDTH, full.height()));
        self.nav(ui, nav_rect, tokens, &mut outcome);

        let main_rect = Rect::from_min_max(pos2(nav_rect.max.x, full.min.y), full.max);
        ui.scope_builder(UiBuilder::new().max_rect(main_rect), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin {
                            left: space::S36 as i8,
                            right: space::S36 as i8,
                            top: 28,
                            bottom: 48,
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(MAIN_MAX_WIDTH);
                            ui.spacing_mut().item_spacing = Vec2::new(space::S8, 20.0);
                            if let Some(error) = controller.profile_error() {
                                widgets::toast(
                                    ui,
                                    tokens,
                                    widgets::Tone::Error,
                                    &format!(
                                        "Your profile file could not be read, so it was left as it is: {error}. Fix or remove the file, then start BuildRail again."
                                    ),
                                );
                            }
                            match self.page {
                                Page::Builds if self.adding => {
                                    self.wizard_page(ui, controller, tokens, &mut inputs)
                                }
                                Page::Builds => self.builds_page(ui, controller, tokens, &mut inputs),
                                Page::Connections => {
                                    self.connections_page(ui, controller, tokens, &mut inputs)
                                }
                                Page::Strip => {
                                    outcome.placement_changed =
                                        self.strip_page(ui, controller, device, tokens)
                                }
                                Page::Share => self.share_page(ui, controller, tokens, &mut inputs),
                                Page::About => self.about_page(ui, controller, tokens),
                            }
                        });
                });
        });

        if !inputs.is_empty() || self.page != Page::Builds {
            // Doing something, or going elsewhere, ends the highlight.
            self.highlight = None;
        }
        let mut commands = Vec::new();
        {
            let ctx = context(controller);
            for input in inputs {
                commands.extend(match input {
                    Input::Wizard(input) => self.wizard.input(input, ctx),
                    Input::Manage(input) => self.manage.input(input, ctx),
                    Input::Import(input) => self.import.input(input, ctx),
                });
            }
        }
        for command in commands {
            controller.execute(command);
        }
        outcome
    }

    /// The navigation column: brand, pages, then Done and Quit at its foot.
    fn nav(&mut self, ui: &mut Ui, rect: Rect, tokens: &Tokens, outcome: &mut SettingsOutcome) {
        ui.painter().rect_filled(rect, 0.0, tokens.nav_fill());
        ui.painter().vline(
            rect.max.x - 0.5,
            rect.y_range(),
            Stroke::new(1.0, tokens.line),
        );
        let inner = rect.shrink2(Vec2::new(12.0, 18.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, space::S2);
            let brand = widgets::text(ui, type_style::SUBHEAD, "BuildRail", tokens.text);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.add(egui::Label::new(egui::WidgetText::Galley(brand)).selectable(false));
            });
            ui.add_space(14.0);
            for (page, name) in Page::ALL {
                let current = self.page == page;
                if nav_item(ui, tokens, name, NavItem::Page(current)).clicked() {
                    self.page = page;
                    if page == Page::Builds {
                        self.adding = false;
                    }
                }
            }
            // Done and Quit sit at the foot, apart from the pages.
            let foot_height = 2.0 * 37.0 + space::S2;
            let room = ui.available_height() - foot_height;
            if room > 0.0 {
                ui.add_space(room);
            }
            if nav_item(ui, tokens, "Done", NavItem::Close).clicked() {
                outcome.close = true;
            }
            if nav_item(ui, tokens, "Quit BuildRail", NavItem::Danger).clicked() {
                outcome.quit = true;
            }
        });
    }

    /// Marks a preference as saved now (for the Saved mark).
    fn mark_saved(&mut self, ui: &Ui) {
        self.saved_at = Some(ui.input(|i| i.time));
    }

    /// Whether the Saved mark shows; keeps a repaint coming until it goes.
    fn saved_visible(&self, ui: &Ui) -> bool {
        let Some(at) = self.saved_at else {
            return false;
        };
        let left = SAVED_FOR - (ui.input(|i| i.time) - at);
        if left > 0.0 {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(left));
            true
        } else {
            false
        }
    }
}

/// Width of the navigation column.
const NAV_WIDTH: f32 = 184.0;
/// The widest the page content grows (a group plus nothing else).
const MAIN_MAX_WIDTH: f32 = widgets::group::MAX_WIDTH;
/// How long the Saved mark stays, in seconds.
const SAVED_FOR: f64 = 2.5;

fn context(controller: &AppController) -> SettingsContext<'_> {
    SettingsContext {
        profile: controller.profile(),
        connections: &controller.prefs().connections,
    }
}

/// "1 build", "3 builds": a count with its noun in agreement.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_agree_with_their_nouns() {
        assert_eq!(count(1, "build", "builds"), "1 build");
        assert_eq!(count(0, "build", "builds"), "0 builds");
        assert_eq!(count(3, "repository", "repositories"), "3 repositories");
    }

    #[test]
    fn pages_are_named_by_what_they_hold() {
        let names: Vec<&str> = Page::ALL.iter().map(|(_, n)| *n).collect();
        assert_eq!(names, ["Builds", "Connections", "Strip", "Share", "About"]);
        let window = SettingsWindow::default();
        assert_eq!(window.page, Page::Builds);
        assert!(!window.is_adding());
        assert!(window.import_preview().is_none());
    }
}
