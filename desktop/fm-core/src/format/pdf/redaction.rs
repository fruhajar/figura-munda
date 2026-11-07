//! Is the blacked-out text actually gone?
//!
//! A redaction drawn as a filled rectangle over live text hides nothing: the
//! text is still in the content stream, in reading order, and selecting the area
//! in any reader copies it out. Finding it means walking the page's operators
//! with enough of a graphics state to know where each piece of text ends up and
//! which rectangles are painted after it.
//!
//! Enough of a graphics state means: the current transformation matrix with its
//! `q`/`Q` stack, the text matrix and the text line matrix. Nothing else affects
//! position, and position is the whole question.

use lopdf::content::Operation;
use lopdf::{Document, Object, ObjectId};

/// A 2-by-3 affine transform, in PDF's own order: `[a b c d e f]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// `self` then `other`, which is the order `cm` composes in.
    fn then(self, other: Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    fn from_operands(operands: &[Object]) -> Option<Matrix> {
        let v: Vec<f64> = operands.iter().filter_map(number).collect();
        (v.len() == 6).then(|| Matrix { a: v[0], b: v[1], c: v[2], d: v[3], e: v[4], f: v[5] })
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn of(points: &[(f64, f64)]) -> Option<Rect> {
        let first = points.first()?;
        let mut r = Rect { x0: first.0, y0: first.1, x1: first.0, y1: first.1 };
        for (x, y) in points {
            r.x0 = r.x0.min(*x);
            r.y0 = r.y0.min(*y);
            r.x1 = r.x1.max(*x);
            r.y1 = r.y1.max(*y);
        }
        Some(r)
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    fn area(&self) -> f64 {
        (self.x1 - self.x0).abs() * (self.y1 - self.y0).abs()
    }
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// Text in a PDF string, as far as a report needs it.
///
/// Simple PDFs use literal strings in a Latin-1-ish encoding, which this decodes
/// directly. A document using a custom font encoding will produce mangled text
/// here; it is still evidence that text is present under the rectangle, which is
/// the finding, and the exact characters matter less than the fact.
fn string_text(o: &Object) -> Option<String> {
    let s = raw_text(o);
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The pieces of a `TJ` array are a single run of text with the spacing numbers
/// between them, so they are joined before trimming rather than after: trimming
/// each piece would weld the words together.
fn raw_text(o: &Object) -> String {
    match o {
        Object::String(bytes, _) => bytes.iter().map(|b| *b as char).collect(),
        Object::Array(items) => items.iter().map(raw_text).collect(),
        _ => String::new(),
    }
}

/// Text found under something that was drawn over it.
pub enum Covered {
    /// Under a filled path. This is a failed redaction.
    ByFill {
        /// Index of the text-showing operator, so it can be removed.
        op_index: usize,
        text: String,
    },
    /// Under a placed image. Could be a scan with an OCR layer, could be a
    /// redaction pasted over as a picture; from here the two look the same.
    ByImage { text: String },
}

struct Shown {
    op_index: usize,
    x: f64,
    y: f64,
    text: String,
}

struct Painted {
    op_index: usize,
    bbox: Rect,
    image: bool,
}

/// Walk one page and report text that something later covers up.
pub fn analyse(doc: &Document, page_id: ObjectId) -> Vec<Covered> {
    let Ok(content) = doc.get_and_decode_page_content(page_id) else { return Vec::new() };
    let page_area = page_bbox(doc, page_id).map(|r| r.area()).unwrap_or(0.0);

    let mut ctm = Matrix::IDENTITY;
    let mut stack: Vec<Matrix> = Vec::new();
    let mut text_matrix = Matrix::IDENTITY;
    let mut line_matrix = Matrix::IDENTITY;
    let mut leading = 0.0f64;
    let mut path: Vec<(f64, f64)> = Vec::new();
    let mut shown: Vec<Shown> = Vec::new();
    let mut painted: Vec<Painted> = Vec::new();

    for (index, Operation { operator, operands }) in content.operations.iter().enumerate() {
        let nums: Vec<f64> = operands.iter().filter_map(number).collect();
        match operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(Matrix::IDENTITY),
            "cm" => {
                if let Some(m) = Matrix::from_operands(operands) {
                    ctm = m.then(ctm);
                }
            }
            "re" if nums.len() == 4 => {
                let (x, y, w, h) = (nums[0], nums[1], nums[2], nums[3]);
                for corner in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
                    path.push(ctm.apply(corner.0, corner.1));
                }
            }
            "m" | "l" if nums.len() == 2 => path.push(ctm.apply(nums[0], nums[1])),
            "c" if nums.len() == 6 => {
                for i in 0..3 {
                    path.push(ctm.apply(nums[i * 2], nums[i * 2 + 1]));
                }
            }
            "v" | "y" if nums.len() == 4 => {
                for i in 0..2 {
                    path.push(ctm.apply(nums[i * 2], nums[i * 2 + 1]));
                }
            }
            // Fills cover what is under them. Strokes do not, so `S` and `s`
            // only end the path.
            "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                if let Some(bbox) = Rect::of(&path) {
                    painted.push(Painted { op_index: index, bbox, image: false });
                }
                path.clear();
            }
            "S" | "s" | "n" => path.clear(),
            "BT" => {
                text_matrix = Matrix::IDENTITY;
                line_matrix = Matrix::IDENTITY;
            }
            "Tm" => {
                if let Some(m) = Matrix::from_operands(operands) {
                    text_matrix = m;
                    line_matrix = m;
                }
            }
            "TL" if !nums.is_empty() => leading = nums[0],
            "Td" if nums.len() == 2 => {
                let shift = Matrix { e: nums[0], f: nums[1], ..Matrix::IDENTITY };
                line_matrix = shift.then(line_matrix);
                text_matrix = line_matrix;
            }
            "TD" if nums.len() == 2 => {
                leading = -nums[1];
                let shift = Matrix { e: nums[0], f: nums[1], ..Matrix::IDENTITY };
                line_matrix = shift.then(line_matrix);
                text_matrix = line_matrix;
            }
            "T*" => {
                let shift = Matrix { e: 0.0, f: -leading, ..Matrix::IDENTITY };
                line_matrix = shift.then(line_matrix);
                text_matrix = line_matrix;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if operator == "'" || operator == "\"" {
                    let shift = Matrix { e: 0.0, f: -leading, ..Matrix::IDENTITY };
                    line_matrix = shift.then(line_matrix);
                    text_matrix = line_matrix;
                }
                if let Some(text) = operands.iter().filter_map(string_text).next() {
                    let origin = text_matrix.then(ctm);
                    let (x, y) = origin.apply(0.0, 0.0);
                    shown.push(Shown { op_index: index, x, y, text });
                }
            }
            "Do" => {
                // An image is placed into the unit square, mapped by the CTM.
                let corners: Vec<(f64, f64)> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                    .iter()
                    .map(|(x, y)| ctm.apply(*x, *y))
                    .collect();
                if let Some(bbox) = Rect::of(&corners) {
                    painted.push(Painted { op_index: index, bbox, image: true });
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for s in shown {
        // Only things painted *after* the text cover it.
        let cover = painted
            .iter()
            .find(|p| p.op_index > s.op_index && p.bbox.contains(s.x, s.y));
        match cover {
            Some(p) if !p.image => {
                out.push(Covered::ByFill { op_index: s.op_index, text: s.text })
            }
            Some(p) => {
                // A full-page image over text is a scan with an OCR layer, which
                // is normal and not a redaction. A patch over part of a page is
                // the suspicious case, so only that is worth reporting.
                let partial = page_area <= 0.0 || p.bbox.area() < page_area * 0.6;
                if partial {
                    out.push(Covered::ByImage { text: s.text });
                }
            }
            None => {}
        }
    }
    out
}

fn page_bbox(doc: &Document, page_id: ObjectId) -> Option<Rect> {
    let dict = doc.get_object(page_id).ok()?.as_dict().ok()?;
    let media = dict
        .get(b"MediaBox")
        .ok()
        .cloned()
        .or_else(|| doc.get_dictionary(page_id).ok().and_then(|d| d.get(b"MediaBox").ok().cloned()))?;
    let media = match media {
        Object::Reference(id) => doc.get_object(id).ok()?.clone(),
        other => other,
    };
    let v: Vec<f64> = media.as_array().ok()?.iter().filter_map(number).collect();
    (v.len() == 4).then(|| Rect { x0: v[0], y0: v[1], x1: v[2], y1: v[3] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let m = Matrix { a: 2.0, b: 0.0, c: 0.0, d: 2.0, e: 10.0, f: 20.0 };
        assert_eq!(m.then(Matrix::IDENTITY), m);
        assert_eq!(Matrix::IDENTITY.then(m), m);
    }

    #[test]
    fn a_scale_then_a_translate_applies_in_that_order() {
        let scale = Matrix { a: 2.0, b: 0.0, c: 0.0, d: 2.0, e: 0.0, f: 0.0 };
        let translate = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 5.0, f: 0.0 };
        assert_eq!(scale.then(translate).apply(1.0, 0.0), (7.0, 0.0));
        assert_eq!(translate.then(scale).apply(1.0, 0.0), (12.0, 0.0));
    }

    #[test]
    fn a_rectangle_bounds_its_corners() {
        let r = Rect::of(&[(10.0, 20.0), (30.0, 5.0)]).unwrap();
        assert!(r.contains(20.0, 10.0));
        assert!(!r.contains(40.0, 10.0));
        assert_eq!(r.area(), 300.0);
    }

    #[test]
    fn latin1_strings_decode_for_the_report() {
        let o = Object::String(b"CZ65 0800".to_vec(), lopdf::StringFormat::Literal);
        assert_eq!(string_text(&o).as_deref(), Some("CZ65 0800"));
        let empty = Object::String(b"   ".to_vec(), lopdf::StringFormat::Literal);
        assert!(string_text(&empty).is_none());
    }

    #[test]
    fn a_show_array_joins_its_pieces() {
        let o = Object::Array(vec![
            Object::String(b"Claimant ".to_vec(), lopdf::StringFormat::Literal),
            Object::Integer(-250),
            Object::String(b"account".to_vec(), lopdf::StringFormat::Literal),
        ]);
        assert_eq!(string_text(&o).as_deref(), Some("Claimant account"));
    }
}
