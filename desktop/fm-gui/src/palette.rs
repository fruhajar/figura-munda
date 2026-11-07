//! Colours, in one place.
//!
//! The classes are the vocabulary of the whole tool, so each one has a colour
//! and keeps it everywhere: the same hue in the file list, the report heading
//! and the diff. Severity is carried by the colour, not by shouting.

use egui::Color32;
use fm_core::model::Class;

pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    pub text: Color32,
    /// Captions and secondary labels.
    pub muted: Color32,
    /// Something is wrong and the user has to decide.
    pub alarm: Color32,
    /// Something was done and checked.
    pub good: Color32,
    /// Kept on purpose.
    pub kept: Color32,
    pub accent: Color32,
    pub line: Color32,
}

pub const DARK: Palette = Palette {
    background: Color32::from_rgb(0x16, 0x18, 0x1c),
    panel: Color32::from_rgb(0x1e, 0x21, 0x26),
    text: Color32::from_rgb(0xe6, 0xe8, 0xea),
    muted: Color32::from_rgb(0x95, 0x9c, 0xa6),
    alarm: Color32::from_rgb(0xe2, 0x7a, 0x6a),
    good: Color32::from_rgb(0x7f, 0xb8, 0x8a),
    kept: Color32::from_rgb(0x8f, 0xa6, 0xc4),
    accent: Color32::from_rgb(0xd4, 0xa5, 0x5f),
    line: Color32::from_rgb(0x2c, 0x30, 0x37),
};

pub const LIGHT: Palette = Palette {
    background: Color32::from_rgb(0xfa, 0xfa, 0xf8),
    panel: Color32::from_rgb(0xff, 0xff, 0xff),
    text: Color32::from_rgb(0x1c, 0x1e, 0x22),
    muted: Color32::from_rgb(0x5c, 0x63, 0x6d),
    alarm: Color32::from_rgb(0xa8, 0x3c, 0x2c),
    good: Color32::from_rgb(0x2f, 0x6b, 0x44),
    kept: Color32::from_rgb(0x3b, 0x5a, 0x80),
    accent: Color32::from_rgb(0x8a, 0x61, 0x1c),
    line: Color32::from_rgb(0xdd, 0xdd, 0xd8),
};

impl Palette {
    /// The colour for a class, by how much it matters.
    ///
    /// A failed redaction and a filesystem timestamp are not the same kind of
    /// problem, and the list has to show that before any text is read.
    pub fn class(&self, class: Class) -> Color32 {
        match class {
            Class::RedactionIntegrity | Class::History | Class::HiddenPayload => self.alarm,
            Class::Location | Class::Identity | Class::Correlation => self.accent,
            Class::Device | Class::Time => self.text,
            Class::StructuralFingerprint | Class::Filesystem => self.muted,
        }
    }

    /// Apply to a context. Widget text colours are set explicitly rather than
    /// left to the default visuals, because the override points differ between
    /// the three widget states and a half-set palette is worse than none.
    pub fn install(&self, ctx: &egui::Context, dark: bool) {
        let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        visuals.panel_fill = self.background;
        visuals.window_fill = self.panel;
        visuals.extreme_bg_color = self.panel;
        visuals.override_text_color = Some(self.text);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, self.line);
        visuals.widgets.inactive.bg_fill = self.panel;
        visuals.widgets.hovered.bg_fill = self.line;
        visuals.widgets.active.bg_fill = self.line;
        visuals.selection.bg_fill = self.accent.linear_multiply(0.35_f32);
        visuals.hyperlink_color = self.kept;
        ctx.set_visuals(visuals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::model::ALL_CLASSES;

    #[test]
    fn every_class_has_a_colour_in_both_themes() {
        for class in ALL_CLASSES {
            assert_ne!(DARK.class(class), Color32::TRANSPARENT);
            assert_ne!(LIGHT.class(class), Color32::TRANSPARENT);
        }
    }

    #[test]
    fn the_worst_classes_are_not_the_colour_of_ordinary_text() {
        for class in [Class::RedactionIntegrity, Class::History, Class::HiddenPayload] {
            assert_eq!(DARK.class(class), DARK.alarm);
            assert_ne!(DARK.class(class), DARK.text);
        }
    }
}
