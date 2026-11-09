//! SVG: an image with no pixels. The browser draws it at whatever size it is asked for. Its own
//! size is the `width` and `height` on its root element (design section 7.6), and the scan counts
//! it at that size. The rest of the file is never read.

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;

use super::raster::RasterHeader;
use super::{DAMAGED_HEADER, NotCovered, Note, SVG_NO_SIZE, TextureFormat};

/// A browser draws a root `svg` element only in this namespace.
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// Each absolute CSS unit as a fraction of px: the unit, then px per unit as a numerator and a
/// denominator (CSS Values 4: 1in = 96px = 2.54cm = 72pt = 6pc, and 1Q = 1/40 cm). No unit is px.
const ABSOLUTE_UNITS: &[(&str, u128, u128)] = &[
    ("", 1, 1),
    ("px", 1, 1),
    ("in", 96, 1),
    ("cm", 4800, 127),
    ("mm", 480, 127),
    ("q", 120, 127),
    ("pt", 4, 3),
    ("pc", 16, 1),
];

/// Reads the own size from the root element. A file with no root `svg` element is invalid. An SVG
/// whose root gives no absolute width and height is unsupported: the game sets its size.
pub fn read(bytes: &[u8]) -> Result<RasterHeader, NotCovered> {
    let damaged = NotCovered::Invalid(DAMAGED_HEADER);
    let mut reader = NsReader::from_reader(bytes);
    loop {
        match reader.read_resolved_event().map_err(|_| damaged)? {
            (namespace, Event::Start(root) | Event::Empty(root)) => {
                let is_svg = namespace == ResolveResult::Bound(Namespace(SVG_NAMESPACE))
                    && root.local_name().as_ref() == "svg";
                if !is_svg {
                    return Err(damaged);
                }
                let (width, height) = own_size(&root)?;
                return Ok(RasterHeader {
                    format: TextureFormat::Svg,
                    width,
                    height,
                    note: Some(Note::SvgOwnSize),
                });
            }
            (_, Event::Eof) => return Err(damaged),
            // The XML declaration, comments and a doctype may come before the root.
            _ => {}
        }
    }
}

/// The root's `width` and `height` in whole px.
fn own_size(root: &BytesStart<'_>) -> Result<(u32, u32), NotCovered> {
    let damaged = NotCovered::Invalid(DAMAGED_HEADER);
    let mut width = None;
    let mut height = None;
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|_| damaged)?;
        let side = match attribute.key.as_ref() {
            "width" => &mut width,
            "height" => &mut height,
            _ => continue,
        };
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| damaged)?;
        *side = absolute_px(&value);
    }
    width
        .zip(height)
        .ok_or(NotCovered::Unsupported(SVG_NO_SIZE))
}

/// A length in an absolute unit, in whole px rounded down, so it is never more than the browser
/// draws. None for anything else: `auto`, a percentage, a unit that depends on the font or the
/// viewport, a number with a sign or an exponent, and a size under 1 px.
fn absolute_px(value: &str) -> Option<u32> {
    let value = value.trim_ascii();
    let unit_at = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(unit_at);
    let &(_, numerator, denominator) = ABSOLUTE_UNITS
        .iter()
        .find(|(name, ..)| unit.eq_ignore_ascii_case(name))?;
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if fraction.contains('.') {
        return None;
    }
    // The number is its digits over 10 to the power of the digits after the point.
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    let point_scale = 10_u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let px = digits.checked_mul(numerator)? / point_scale.checked_mul(denominator)?;
    u32::try_from(px).ok().filter(|&px| px > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_with_no_unit_or_px_is_px() {
        assert_eq!(absolute_px("80"), Some(80));
        assert_eq!(absolute_px("80px"), Some(80));
        assert_eq!(absolute_px(" 80PX "), Some(80));
    }

    #[test]
    fn absolute_units_convert_at_the_css_ratios() {
        assert_eq!(absolute_px("1in"), Some(96));
        assert_eq!(absolute_px("2.54cm"), Some(96));
        assert_eq!(absolute_px("25.4mm"), Some(96));
        assert_eq!(absolute_px("101.6Q"), Some(96));
        assert_eq!(absolute_px("72pt"), Some(96));
        assert_eq!(absolute_px("6pc"), Some(96));
    }

    #[test]
    fn a_part_of_a_px_rounds_down() {
        assert_eq!(absolute_px("97.9"), Some(97));
        assert_eq!(absolute_px("1cm"), Some(37));
        assert_eq!(absolute_px("5."), Some(5));
    }

    #[test]
    fn a_length_that_needs_the_page_or_the_font_gives_no_size() {
        for value in ["auto", "100%", "10em", "10rem", "10ex", "50vw", ""] {
            assert_eq!(absolute_px(value), None, "{value}");
        }
    }

    #[test]
    fn a_number_that_is_not_plain_decimal_gives_no_size() {
        for value in ["-10", "+10", "1e3", "1.2.3", ".", "1 px"] {
            assert_eq!(absolute_px(value), None, "{value}");
        }
    }

    #[test]
    fn a_size_under_one_px_or_too_big_for_a_side_gives_no_size() {
        assert_eq!(absolute_px("0"), None);
        assert_eq!(absolute_px("0.5"), None);
        assert_eq!(absolute_px("4294967296"), None);
        assert_eq!(absolute_px("1".repeat(50).as_str()), None);
    }
}
