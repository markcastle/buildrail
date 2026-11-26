//! The Task Rail widgets on their own, with `egui_kittest`: each reports
//! its role, name and state to assistive technology and does its one job
//! from a click or the keyboard.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use buildrail_app::ui::fonts::{self, MemoryFontSource, Platform};
use buildrail_app::ui::glyphs::paint_plus;
use buildrail_app::ui::theme::{DARK, LIGHT};
use buildrail_app::ui::widgets::{
    back_button, button, choice_cards, connector_card, display_picker, error_line, group,
    icon_button, list_row, nav_item, page_header, saved_mark, segmented, subhead, toast, toggle,
    ButtonKind, CardAction, Choice, ConnectorCard, DisplayTile, NavItem, TextField, Tone,
};
use buildrail_app::ui::Tokens;
use buildrail_platform::geometry::Edge;
use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{self, Key};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use support::CpuRenderer;

#[derive(Default)]
struct State {
    dark: bool,
    on: bool,
    choice: u8,
    text: String,
    secret: String,
    clicks: Vec<String>,
    page: u8,
    fonts: bool,
}

fn draw(ui: &mut egui::Ui, s: &mut State) {
    if !s.fonts {
        // The weight families exist from the next pass on.
        s.fonts = true;
        fonts::install(ui.ctx(), &MemoryFontSource::new(), &Platform::Linux);
        return;
    }
    let t: Tokens = if s.dark { DARK } else { LIGHT };
    egui::ScrollArea::vertical().show(ui, |ui| {
        page_header(ui, &t, "Widgets", "Every widget, once.");
        subhead(ui, &t, "Controls");
        let mut clicks = Vec::new();
        group(ui, &t, |g| {
            g.row("Start at login", Some("A switch."), |ui| {
                toggle(ui, &t, &mut s.on, "Start at login");
            });
            g.row("Theme", None, |ui| {
                segmented(
                    ui,
                    &t,
                    "Theme",
                    &mut s.choice,
                    &[(0, "System"), (1, "Light"), (2, "Dark")],
                );
            });
            g.label("Fields");
            g.stack("Name", Some("A field."), |ui| {
                TextField::new(&mut s.text, "Name")
                    .placeholder("work")
                    .show(ui, &t);
                TextField::new(&mut s.secret, "Secret")
                    .password(true)
                    .multiline(1)
                    .enabled(false)
                    .width(200.0)
                    .show(ui, &t);
            });
            g.block(|ui| {
                for (kind, label) in [
                    (ButtonKind::Primary, "Primary"),
                    (ButtonKind::Plain, "Plain"),
                    (ButtonKind::Quiet, "Quiet"),
                    (ButtonKind::QuietDanger, "Danger"),
                ] {
                    if button(ui, &t, kind, label, true, false).clicked() {
                        clicks.push(label.to_owned());
                    }
                }
                button(ui, &t, ButtonKind::Plain, "Disabled", false, true);
                if back_button(ui, &t, "Back").clicked() {
                    clicks.push("Back".into());
                }
                if icon_button(ui, &t, "Add", "Add a thing", &|p, c, size, color| {
                    paint_plus(p, c, size, color)
                })
                .clicked()
                {
                    clicks.push("Add".into());
                }
            });
            g.flush(|ui| {
                if list_row(ui, &t, "acme/keevoro", Some("Archived"), None, false).clicked() {
                    clicks.push("row".into());
                }
                list_row(ui, &t, "main", None, Some(true), true);
            });
        });
        for (i, name) in ["Builds", "Strip"].iter().enumerate() {
            if nav_item(ui, &t, name, NavItem::Page(s.page == i as u8)).clicked() {
                s.page = i as u8;
            }
        }
        nav_item(ui, &t, "Done", NavItem::Close);
        nav_item(ui, &t, "Quit", NavItem::Danger);
        let choices = [
            Choice {
                title: "GitHub",
                text: "One.",
            },
            Choice {
                title: "Cloudflare",
                text: "Two.",
            },
        ];
        if let Some(i) = choice_cards(ui, &t, &choices, Some(0)) {
            clicks.push(format!("choice {i}"));
        }
        let card = ConnectorCard {
            dot: t.done,
            name: "work",
            count: Some("1 build".into()),
            count_color: None,
            facts: vec!["GitHub".into(), "octocat".into()],
            error: Some("GitHub refused this token.".into()),
        };
        if let Some(i) = connector_card(
            ui,
            &t,
            &card,
            &[
                CardAction::new("Sync").tooltip("Now".into()),
                CardAction::new("Off").enabled(false),
                CardAction::danger("Remove"),
            ],
        ) {
            clicks.push(format!("card {i}"));
        }
        let tiles = [
            DisplayTile {
                name: "Display 1".into(),
                width: 2560,
                height: 1440,
            },
            DisplayTile {
                name: "Display 2".into(),
                width: 1080,
                height: 1920,
            },
        ];
        if let Some(i) = display_picker(ui, &t, &tiles, Some(0), Edge::Bottom) {
            clicks.push(format!("display {i}"));
        }
        if let Some(i) = display_picker(ui, &t, &tiles[..1], Some(0), Edge::Top) {
            clicks.push(format!("top display {i}"));
        }
        toast(ui, &t, Tone::Info, "Profile copied.");
        toast(ui, &t, Tone::Success, "Saved.");
        toast(ui, &t, Tone::Error, "Could not save.");
        error_line(ui, &t, "Something went wrong.");
        saved_mark(ui, &t, true);
        saved_mark(ui, &t, false);
        s.clicks.extend(clicks);
    });
}

fn harness(dark: bool) -> Harness<'static, State> {
    Harness::builder()
        .with_size([800.0, 2400.0])
        .renderer(CpuRenderer::default())
        .build_ui_state(
            draw,
            State {
                dark,
                ..State::default()
            },
        )
}

fn control<'a>(h: &'a Harness<'_, State>, name: &'a str, role: Role) -> egui_kittest::Node<'a> {
    h.query_all_by_label(name)
        .find(|n| n.accesskit_node().role() == role)
        .unwrap_or_else(|| panic!("no {role:?} {name:?}"))
}

#[test]
fn toggle_flips_and_reports_its_state() {
    let mut h = harness(false);
    h.run();
    let switch = control(&h, "Start at login", Role::Switch);
    assert_eq!(switch.accesskit_node().toggled(), Some(Toggled::False));
    switch.click();
    h.run();
    assert!(h.state().on);
    assert_eq!(
        control(&h, "Start at login", Role::Switch)
            .accesskit_node()
            .toggled(),
        Some(Toggled::True)
    );
    // The keyboard flips it too, and focus shows the ring.
    control(&h, "Start at login", Role::Switch).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert!(!h.state().on);
}

#[test]
fn segmented_selects_one_option() {
    let mut h = harness(true);
    h.run();
    assert!(h
        .query_by_role_and_label(Role::RadioGroup, "Theme")
        .is_some());
    control(&h, "Dark", Role::RadioButton).click();
    h.run();
    assert_eq!(h.state().choice, 2);
    assert_eq!(
        control(&h, "Dark", Role::RadioButton)
            .accesskit_node()
            .toggled(),
        Some(Toggled::True)
    );
    control(&h, "Light", Role::RadioButton).focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().choice, 1);
}

#[test]
fn buttons_fire_and_disabled_ones_do_not() {
    let mut h = harness(false);
    h.run();
    for name in [
        "Primary", "Plain", "Quiet", "Danger", "Back", "Add", "Disabled",
    ] {
        h.query_all_by_label(name)
            .find(|n| n.accesskit_node().role() == Role::Button)
            .unwrap_or_else(|| panic!("no {name}"))
            .click();
        h.run();
    }
    let clicks = &h.state().clicks;
    for name in ["Primary", "Plain", "Quiet", "Danger", "Back", "Add"] {
        assert!(clicks.contains(&name.to_owned()), "{name}: {clicks:?}");
    }
    assert!(!clicks.contains(&"Disabled".to_owned()));
    assert!(control(&h, "Disabled", Role::Button)
        .accesskit_node()
        .is_disabled());
    // Hover paints the hover states without changing anything.
    control(&h, "Quiet", Role::Button).hover();
    h.run();
    control(&h, "Add", Role::Button).focus();
    h.run();
}

#[test]
fn nav_switches_page_and_cards_report_choices() {
    let mut h = harness(true);
    h.run();
    control(&h, "Strip", Role::Tab).click();
    h.run();
    assert_eq!(h.state().page, 1);
    assert_eq!(
        control(&h, "Strip", Role::Tab).accesskit_node().toggled(),
        Some(Toggled::True)
    );
    control(&h, "Cloudflare", Role::RadioButton).click();
    h.run();
    control(&h, "acme/keevoro, Archived", Role::Button).click();
    h.run();
    control(&h, "Remove", Role::Button).click();
    h.run();
    control(&h, "Display 2, 1080 × 1920", Role::RadioButton).click();
    h.run();
    let clicks = &h.state().clicks;
    for expected in ["choice 1", "row", "card 2", "display 1"] {
        assert!(
            clicks.contains(&expected.to_owned()),
            "{expected}: {clicks:?}"
        );
    }
    assert!(h.query_by_label("GitHub refused this token.").is_some());
    assert!(h.query_by_label("Saved").is_some());
}

#[test]
fn text_fields_take_typing_and_name_themselves() {
    let mut h = harness(false);
    h.run();
    let name = control(&h, "Name", Role::TextInput);
    name.focus();
    h.run();
    control(&h, "Name", Role::TextInput).type_text("work");
    h.run();
    assert_eq!(h.state().text, "work");
    let secret = h
        .query_all_by_label("Secret")
        .find(|n| n.accesskit_node().role() != Role::Label)
        .unwrap();
    assert!(secret.accesskit_node().is_disabled());
    // Both themes render.
    let _ = h.render().unwrap();
    let mut dark = harness(true);
    dark.run();
    let _ = dark.render().unwrap();
}
