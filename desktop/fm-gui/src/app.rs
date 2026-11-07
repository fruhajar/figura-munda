//! The window.
//!
//! Three columns: what you chose, the file list, and the detail of one file.
//! The detail is where the four artefacts live, as tabs: the report, the diff,
//! the recipient view, and the profile table that produced them.

use crate::palette::{Palette, DARK, LIGHT};
use crate::session::{DetailTab, Destination, Item, ItemState, Session, Stage};
use crate::work::{Job, Worker};
use egui::{Align, Color32, Layout, RichText, ScrollArea, Ui};
use fm_core::model::ALL_CLASSES;
use fm_core::profile::{Blending, Profile};
use fm_core::report::{self, RecipientView, Report};
use std::path::PathBuf;

/// What the footer needs to draw, copied out of the session so that the buttons
/// can act on the worker without a live borrow of what it owns.
struct FootView {
    headline: String,
    can_review: bool,
    can_run: bool,
    why_not_run: Option<String>,
    destination: String,
    acknowledged: bool,
    inert_profile: bool,
    content_loss: Vec<(String, String)>,
}

/// What the user asked for, applied once the drawing is finished.
enum Intent {
    None,
    Review,
    Run,
    Back,
    Clear,
    Acknowledge(bool),
}

pub struct App {
    worker: Worker,
    dark: bool,
    /// Shown instead of the file list while the profile is being edited.
    editing_profile: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        let dark = cc.egui_ctx.style().visuals.dark_mode;
        let app = App { worker: Worker::new(Session::default()), dark, editing_profile: false };
        app.palette().install(&cc.egui_ctx, dark);
        app
    }

    fn palette(&self) -> &'static Palette {
        if self.dark { &DARK } else { &LIGHT }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.worker.poll() {
            ctx.request_repaint();
        }
        let palette = self.palette();

        egui::TopBottomPanel::top("head").show(ctx, |ui| self.draw_head(ui, palette, ctx));
        egui::TopBottomPanel::bottom("foot").show(ctx, |ui| self.draw_foot(ui, palette, ctx));
        egui::SidePanel::left("left").min_width(250.0).show(ctx, |ui| {
            if self.editing_profile {
                self.draw_profile_editor(ui, palette);
            } else {
                self.draw_sources(ui, palette);
            }
        });
        egui::SidePanel::left("list").min_width(300.0).show(ctx, |ui| self.draw_list(ui, palette));
        egui::CentralPanel::default().show(ctx, |ui| self.draw_detail(ui, palette));
    }
}

impl App {
    fn draw_head(&mut self, ui: &mut Ui, palette: &Palette, ctx: &egui::Context) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Figura Munda").color(palette.text));
            ui.label(
                RichText::new("shows you what a file reveals about you, then removes it")
                    .color(palette.muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(if self.dark { "light" } else { "dark" }).clicked() {
                    self.dark = !self.dark;
                    self.palette().install(ctx, self.dark);
                }
            });
        });
        ui.add_space(4.0);
        let stage = self.worker.stage();
        ui.horizontal(|ui| {
            for (index, s) in [Stage::Choose, Stage::Review, Stage::Done].iter().enumerate() {
                let reached = stage_order(stage) >= stage_order(*s);
                let colour = if reached { palette.accent } else { palette.muted };
                if index > 0 {
                    ui.label(RichText::new("→").color(palette.muted));
                }
                ui.label(RichText::new(s.title()).color(colour));
            }
        });
        ui.add_space(6.0);
    }

    fn draw_foot(&mut self, ui: &mut Ui, palette: &Palette, ctx: &egui::Context) {
        ui.add_space(6.0);
        let busy = self.worker.busy();
        let stage = self.worker.stage();

        // Read everything the footer needs, then let go of the session: the
        // buttons act on the worker, and holding a borrow across the closures
        // that draw them would mean two owners of the same thing.
        let view = self.worker.get().map(|session| FootView {
            headline: session.headline(),
            can_review: session.can_review(),
            can_run: session.can_run(),
            why_not_run: session.why_not_run(),
            destination: session.destination.label(),
            acknowledged: session.acknowledged_content_loss,
            inert_profile: session.profile.inert() && !session.items.is_empty(),
            content_loss: session
                .content_loss()
                .into_iter()
                .map(|(path, what)| {
                    (
                        path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                        what.to_string(),
                    )
                })
                .collect(),
        });

        let fallback = match stage {
            Stage::Reviewing => "Reading the files. Nothing is being written.",
            _ => "Writing.",
        };
        ui.horizontal_wrapped(|ui| {
            if busy {
                ui.spinner();
            }
            let text = view.as_ref().map(|v| v.headline.clone()).unwrap_or_else(|| fallback.to_string());
            ui.label(RichText::new(text).color(palette.text));
        });

        let Some(view) = view else {
            ui.add_space(6.0);
            return;
        };

        let mut intent = Intent::None;

        // The gate. Everything a run would destroy, listed, before it happens.
        if stage == Stage::Review && !view.content_loss.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new("This would remove things a reader can see, not only metadata:")
                    .color(palette.alarm),
            );
            ScrollArea::vertical().max_height(90.0).id_salt("loss").show(ui, |ui| {
                for (name, what) in &view.content_loss {
                    ui.label(RichText::new(format!("{name}  —  {what}")).color(palette.muted));
                }
            });
            let mut acknowledged = view.acknowledged;
            if ui
                .checkbox(&mut acknowledged, "I have read that list and want it removed")
                .changed()
            {
                intent = Intent::Acknowledge(acknowledged);
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| match stage {
            Stage::Choose => {
                if ui
                    .add_enabled(view.can_review && !busy, egui::Button::new("Look at these files"))
                    .clicked()
                {
                    intent = Intent::Review;
                }
                if view.inert_profile {
                    ui.label(
                        RichText::new(
                            "The inspect profile changes nothing, so there is nothing to run.",
                        )
                        .color(palette.muted),
                    );
                }
            }
            Stage::Review => {
                let label = format!("Clean {}", view.destination);
                if ui.add_enabled(view.can_run && !busy, egui::Button::new(label)).clicked() {
                    intent = Intent::Run;
                }
                if ui.button("Back").clicked() {
                    intent = Intent::Back;
                }
                if let Some(why) = &view.why_not_run {
                    ui.label(RichText::new(why).color(palette.muted));
                }
            }
            Stage::Done => {
                if ui.button("Start again").clicked() {
                    intent = Intent::Clear;
                }
            }
            _ => {
                ui.label(RichText::new("Working\u{2026}").color(palette.muted));
            }
        });
        ui.add_space(6.0);

        match intent {
            Intent::None => {}
            Intent::Review => self.worker.start(Job::Review, ctx.clone()),
            Intent::Run => self.worker.start(Job::Run, ctx.clone()),
            Intent::Back => {
                if let Some(s) = self.worker.get_mut() {
                    s.back_to_choosing();
                }
            }
            Intent::Clear => {
                if let Some(s) = self.worker.get_mut() {
                    s.clear();
                }
            }
            Intent::Acknowledge(value) => {
                if let Some(s) = self.worker.get_mut() {
                    s.acknowledged_content_loss = value;
                }
            }
        }
    }

    fn draw_sources(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.add_space(6.0);
        ui.label(RichText::new("Files").color(palette.muted));

        let busy = self.worker.busy();
        let mut chosen: Vec<PathBuf> = Vec::new();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Add files…")).clicked() {
                if let Some(files) = rfd::FileDialog::new().pick_files() {
                    chosen.extend(files);
                }
            }
            if ui.add_enabled(!busy, egui::Button::new("Add folder…")).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    chosen.push(dir);
                }
            }
        });

        // Dropping onto the window is the normal way to use this.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        chosen.extend(dropped);

        let Some(session) = self.worker.get_mut() else {
            ui.add_space(6.0);
            return;
        };
        if !chosen.is_empty() {
            session.add_paths(&chosen);
        }
        let mut recursive = session.recursive;
        if ui.checkbox(&mut recursive, "include sub-folders").changed() {
            session.recursive = recursive;
        }
        if !session.skipped.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} file(s) skipped: not a format this tool handles",
                    session.skipped.len()
                ))
                .color(palette.muted),
            );
        }

        ui.add_space(10.0);
        ui.separator();
        ui.label(RichText::new("Profile").color(palette.muted));
        for builtin in Profile::builtins() {
            let selected = session.profile.name == builtin.name;
            let text = RichText::new(&builtin.name)
                .color(if selected { palette.accent } else { palette.text });
            if ui.selectable_label(selected, text).clicked() && !selected {
                session.set_profile(builtin.clone());
            }
        }
        ui.label(RichText::new(&session.profile.description).color(palette.muted));
        ui.add_space(4.0);
        if ui.button(if self.editing_profile { "Hide the table" } else { "Change a line" }).clicked() {
            self.editing_profile = !self.editing_profile;
        }

        ui.add_space(10.0);
        ui.separator();
        ui.label(RichText::new("Where the cleaned files go").color(palette.muted));
        let mut destination = session.destination.clone();
        for option in [Destination::Beside, Destination::InPlace] {
            let selected = destination == option;
            if ui.selectable_label(selected, option.label()).clicked() {
                destination = option;
            }
        }
        let folder_selected = matches!(destination, Destination::Folder(_));
        if ui.selectable_label(folder_selected, "into a folder I choose…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                destination = Destination::Folder(dir);
            }
        }
        if let Destination::Folder(dir) = &destination {
            ui.label(RichText::new(dir.display().to_string()).color(palette.muted));
        }
        if destination != session.destination {
            session.destination = destination;
        }
        if session.destination == Destination::InPlace {
            ui.label(
                RichText::new(
                    "The originals will be replaced. There will be nothing left to compare against.",
                )
                .color(palette.alarm),
            );
        }
        ui.add_space(6.0);
    }

    /// The profile table: the single decision, with every cell visible.
    fn draw_profile_editor(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.add_space(6.0);
        ui.label(RichText::new("What happens to each kind of thing").color(palette.muted));
        let Some(session) = self.worker.get_mut() else { return };
        let mut profile = session.profile.clone();
        let mut edited = false;

        ScrollArea::vertical().id_salt("profile").show(ui, |ui| {
            for class in ALL_CLASSES {
                ui.add_space(4.0);
                ui.label(RichText::new(class.title()).color(palette.class(class)));
                ui.horizontal_wrapped(|ui| {
                    for action in fm_core::model::ALL_ACTIONS {
                        if !class.allows(action) {
                            continue;
                        }
                        let selected = profile.action(class) == action;
                        if ui.selectable_label(selected, action.label()).clicked() && !selected {
                            profile.actions.insert(class, action);
                            profile.name = "custom".to_string();
                            profile.description =
                                "Your own table, changed from a built-in profile.".to_string();
                            edited = true;
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.separator();
            ui.label(RichText::new("How much to blend in").color(palette.muted));
            for blending in [Blending::MatchCommon, Blending::Minimal] {
                let selected = profile.blending == blending;
                if ui.selectable_label(selected, blending.label()).clicked() && !selected {
                    profile.blending = blending;
                    edited = true;
                }
            }
            ui.label(RichText::new(profile.blending.explanation()).color(palette.muted));
        });

        if edited {
            session.set_profile(profile);
        }
        ui.add_space(6.0);
        if ui.button("Done").clicked() {
            self.editing_profile = false;
        }
    }

    fn draw_list(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.add_space(6.0);
        let Some(session) = self.worker.get_mut() else {
            ui.label(RichText::new("Working…").color(palette.muted));
            return;
        };
        if session.items.is_empty() {
            ui.label(RichText::new("No files yet.").color(palette.muted));
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} file(s)", session.items.len())).color(palette.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    session.items.clear();
                    session.selected = None;
                }
            });
        });
        ui.separator();

        let mut select: Option<usize> = None;
        let mut drop: Option<usize> = None;
        let mut toggles: Vec<(usize, bool)> = Vec::new();
        ScrollArea::vertical().id_salt("files").show(ui, |ui| {
            for (index, item) in session.items.iter().enumerate() {
                let selected = session.selected == Some(index);
                ui.horizontal(|ui| {
                    let refused = matches!(item.state, ItemState::Refused(_));
                    let mut include = item.include;
                    if ui.add_enabled(!refused, egui::Checkbox::without_text(&mut include)).changed() {
                        toggles.push((index, include));
                    }
                    let name = item
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if ui
                        .selectable_label(selected, RichText::new(name).color(palette.text))
                        .clicked()
                    {
                        select = Some(index);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            drop = Some(index);
                        }
                    });
                });
                ui.label(RichText::new(item.headline()).color(headline_colour(item, palette)).small());
                ui.label(RichText::new(item.kind_and_size()).color(palette.muted).small());
                ui.add_space(2.0);
            }
        });
        for (index, include) in toggles {
            session.items[index].include = include;
        }
        if let Some(index) = select {
            session.selected = Some(index);
        }
        if let Some(index) = drop {
            session.remove(index);
        }
    }

    fn draw_detail(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(session) = self.worker.get_mut() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Working…").color(palette.muted));
            });
            return;
        };
        let mut tab = session.tab;
        ui.horizontal(|ui| {
            for option in [
                DetailTab::Report,
                DetailTab::Diff,
                DetailTab::Recipient,
                DetailTab::Profile,
            ] {
                let label = match option {
                    DetailTab::Report => "What is in it",
                    DetailTab::Diff => "What would change",
                    DetailTab::Recipient => "What they would see",
                    DetailTab::Profile => "The profile",
                };
                if ui.selectable_label(tab == option, label).clicked() {
                    tab = option;
                }
            }
        });
        session.tab = tab;
        ui.separator();

        if tab == DetailTab::Profile {
            ScrollArea::vertical().id_salt("profiletext").show(ui, |ui| {
                ui.label(RichText::new(report::text_profile(&session.profile)).monospace());
            });
            return;
        }

        let Some(item) = session.selected_item() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Pick a file on the left, or look at the files first.")
                        .color(palette.muted),
                );
            });
            return;
        };

        ScrollArea::vertical().id_salt("detail").show(ui, |ui| match &item.state {
            ItemState::Waiting => {
                ui.label(
                    RichText::new("Not read yet. Nothing has been opened.").color(palette.muted),
                );
            }
            ItemState::Failed(why) => {
                ui.label(RichText::new(format!("This file could not be read: {why}")).color(palette.alarm));
            }
            ItemState::Refused(why) => {
                ui.label(RichText::new("Left untouched on purpose").color(palette.kept));
                ui.add_space(4.0);
                ui.label(RichText::new(why).color(palette.text));
            }
            ItemState::Written { to, bytes, verified } => {
                let colour = if *verified { palette.good } else { palette.alarm };
                ui.label(
                    RichText::new(if *verified {
                        "Cleaned, and the result was checked."
                    } else {
                        "Written, but the check did not pass. Read this before sending it."
                    })
                    .color(colour),
                );
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{}", to.display())).color(palette.muted));
                ui.label(RichText::new(format!("{bytes} bytes")).color(palette.muted));
            }
            ItemState::Previewed(preview) => match tab {
                DetailTab::Report => draw_report(ui, palette, &preview.inspection),
                DetailTab::Diff => {
                    ui.label(RichText::new(preview.counts.sentence()).color(palette.text));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(report::text_diff(&preview.changes, &session.profile))
                            .monospace(),
                    );
                    ui.add_space(6.0);
                    let colour = if preview.verified { palette.good } else { palette.alarm };
                    ui.label(
                        RichText::new(if preview.verified {
                            "Checked: everything this profile promises to remove would be gone."
                        } else {
                            "The check did not pass on this file. Report it rather than trusting it."
                        })
                        .color(colour),
                    );
                    if let Some(sig) = preview.signature {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "Afterwards it would look like {} ({}, {}).",
                                sig.name,
                                sig.crowd.label(),
                                sig.provenance.label()
                            ))
                            .color(palette.kept),
                        );
                        ui.label(RichText::new(sig.note).color(palette.muted).small());
                    } else {
                        ui.label(
                            RichText::new(
                                "Afterwards it would not match any shape on record, so it may \
                                 stand out. Nothing would be leaking; it is the pattern of what \
                                 remains that is unusual.",
                            )
                            .color(palette.muted),
                        );
                    }
                }
                DetailTab::Recipient => {
                    // Only what survives. Re-inspecting the cleaned bytes would
                    // be exact, but they are not kept, so this is derived from
                    // the findings the diff leaves behind.
                    let removed: Vec<&str> =
                        preview.changes.iter().map(|c| c.locator.as_str()).collect();
                    let mut survives = preview.inspection.clone();
                    survives.findings.retain(|f| !removed.contains(&f.locator.as_str()));
                    let view = RecipientView::of(&survives);
                    ui.label(RichText::new(view.sentence()).color(palette.text));
                    ui.add_space(8.0);
                    for (class, findings) in &view.visible {
                        ui.label(RichText::new(class.title()).color(palette.class(*class)));
                        for f in findings {
                            ui.label(
                                RichText::new(format!(
                                    "    {}  {}",
                                    f.label,
                                    f.value.as_deref().unwrap_or("present")
                                ))
                                .color(palette.muted),
                            );
                        }
                        ui.add_space(4.0);
                    }
                    if view.visible.is_empty() {
                        ui.label(
                            RichText::new("Nothing at all, beyond the contents of the file.")
                                .color(palette.good),
                        );
                    }
                }
                DetailTab::Profile => {}
            },
        });
    }
}

fn draw_report(ui: &mut Ui, palette: &Palette, inspection: &fm_core::Inspection) {
    let report = Report::new(inspection);
    ui.label(RichText::new(report.headline()).color(palette.text).size(16.0));
    ui.add_space(4.0);
    ui.label(RichText::new(report.counts.sentence()).color(palette.muted));
    for blocker in &inspection.blockers {
        ui.add_space(6.0);
        ui.label(RichText::new(&blocker.reason).color(palette.alarm));
        ui.label(RichText::new(&blocker.detail).color(palette.text));
    }
    ui.add_space(8.0);
    for (class, _) in report.grouped() {
        ui.label(RichText::new(class.title()).color(palette.class(class)));
        ui.label(RichText::new(class.sentence()).color(palette.muted).small());
        for row in report.rows(class) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(row.label).color(palette.text));
                ui.label(
                    RichText::new(row.value.unwrap_or("present"))
                        .color(palette.muted)
                        .monospace(),
                );
                if let Some(places) = row.places_note() {
                    ui.label(RichText::new(places).color(palette.muted).small());
                }
                if row.protected {
                    ui.label(RichText::new("kept on purpose").color(palette.kept).small());
                }
                if row.advisory {
                    ui.label(RichText::new("reported only").color(palette.kept).small());
                }
            });
            if let Some(note) = row.note {
                ui.label(RichText::new(note).color(palette.muted).small());
            }
        }
        ui.add_space(8.0);
    }
}

fn headline_colour(item: &Item, palette: &Palette) -> Color32 {
    match &item.state {
        ItemState::Failed(_) => palette.alarm,
        ItemState::Refused(_) => palette.kept,
        ItemState::Written { verified: false, .. } => palette.alarm,
        ItemState::Written { .. } => palette.good,
        ItemState::Previewed(p) => match Report::new(&p.inspection).worst() {
            Some(f) => palette.class(f.class),
            None => palette.good,
        },
        ItemState::Waiting => palette.muted,
    }
}

fn stage_order(stage: Stage) -> u8 {
    match stage {
        Stage::Choose => 0,
        Stage::Reviewing => 1,
        Stage::Review => 1,
        Stage::Running => 2,
        Stage::Done => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_indicator_only_moves_forward() {
        assert!(stage_order(Stage::Choose) < stage_order(Stage::Review));
        assert!(stage_order(Stage::Review) < stage_order(Stage::Done));
        assert_eq!(stage_order(Stage::Reviewing), stage_order(Stage::Review));
    }

    #[test]
    fn every_class_has_a_tab_label_and_a_colour() {
        for class in ALL_CLASSES {
            assert!(!class.title().is_empty());
            assert_ne!(DARK.class(class), Color32::TRANSPARENT);
        }
    }
}
