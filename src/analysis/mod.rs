use serde::{Deserialize, Serialize};

/// Represents a region of interest on the screen
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Approximate selected-content location, validated before answer composition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionCenter {
    x: f64,
    y: f64,
}

impl SelectionCenter {
    pub fn normalized(self) -> [f64; 2] {
        [self.x, self.y]
    }

    pub fn from_pixels(x: f64, y: f64, width: u32, height: u32) -> Option<Self> {
        if width == 0
            || height == 0
            || !x.is_finite()
            || !y.is_finite()
            || !(0.0..=f64::from(width)).contains(&x)
            || !(0.0..=f64::from(height)).contains(&y)
        {
            return None;
        }
        Some(Self {
            x: if x == 0.0 { 0.0 } else { x / f64::from(width) },
            y: if y == 0.0 { 0.0 } else { y / f64::from(height) },
        })
    }
}

/// A short description of the selected source, separate from the user's question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ReaderSourceSubject(String);

impl ReaderSourceSubject {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReaderSourceSubject {
    type Error = &'static str;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        if text.is_empty()
            || text.len() > 160
            || text != text.trim()
            || text.eq_ignore_ascii_case("NONE")
            || text
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        {
            return Err("invalid Reader source subject");
        }
        Ok(Self(text))
    }
}

/// How the proposal described a fresh circled or highlighted selection.
pub enum ReaderSubject {
    Recognized(ReaderSourceSubject),
    Uncertain,
}

pub(crate) struct ReaderSelection {
    pub(crate) center: SelectionCenter,
    pub(crate) subject: Option<ReaderSubject>,
}

impl ReaderSelection {
    pub(crate) fn recognized_subject(&self) -> Option<&ReaderSourceSubject> {
        match &self.subject {
            Some(ReaderSubject::Recognized(subject)) => Some(subject),
            _ => None,
        }
    }
}

impl std::fmt::Display for SelectionCenter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let axis = |value: f64| {
            format!("{value:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned()
        };
        write!(formatter, "({}, {})", axis(self.x), axis(self.y))
    }
}

#[cfg(test)]
mod tests {
    use super::SelectionCenter;

    #[test]
    fn equal_screen_proportions_have_equal_compact_tags() {
        for (width, height) in [(768, 1024), (1404, 1872), (1632, 2154)] {
            let center = SelectionCenter::from_pixels(
                f64::from(width) / 2.0,
                f64::from(height) / 2.0,
                width,
                height,
            )
            .unwrap();
            assert_eq!(center.to_string(), "(0.5, 0.5)");
        }
        assert_eq!(
            SelectionCenter::from_pixels(-0.0, 1024.0, 768, 1024)
                .unwrap()
                .to_string(),
            "(0, 1)"
        );
        assert_eq!(
            SelectionCenter::from_pixels(384.0, 230.0, 768, 1024)
                .unwrap()
                .to_string(),
            "(0.5, 0.22)"
        );
    }

    #[test]
    fn invalid_coordinates_or_dimensions_never_become_tags() {
        for (x, y, w, h) in [
            (f64::NAN, 1.0, 768, 1024),
            (1.0, f64::INFINITY, 768, 1024),
            (-1.0, 1.0, 768, 1024),
            (769.0, 1.0, 768, 1024),
            (1.0, 1025.0, 768, 1024),
            (1.0, 1.0, 0, 1024),
            (1.0, 1.0, 768, 0),
        ] {
            assert!(SelectionCenter::from_pixels(x, y, w, h).is_none());
        }
    }
}
