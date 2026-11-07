//! HTML: inline scripts, attributes that name files, and inline CSS (design section 8.1).

use std::ops::Range;

use html5gum::{DefaultEmitter, StartTag, Token, Tokenizer};

/// What one HTML file gives the bundle scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    /// The inline scripts a browser runs as JavaScript, as byte ranges of the file.
    pub scripts: Vec<Range<usize>>,
    /// The inline scripts that hold data, such as an import map or JSON, as byte ranges of the
    /// file.
    pub data_blocks: Vec<Range<usize>>,
    /// Values that can name a URL: `src`, each `srcset` candidate, `poster`, `data`, `href` on a
    /// `<link>` that loads something, and `href` on the SVG elements that load an image. Not
    /// `<a href>` or `<link rel=canonical>`: they load nothing.
    pub references: Vec<String>,
    /// The text of `<style>` elements and `style` attributes.
    pub styles: Vec<String>,
}

/// A `<script>` start tag whose end tag has not been read yet.
struct OpenScript {
    /// The byte after the start tag.
    text_start: usize,
    kind: InlineText,
}

/// What a browser does with the text inside a `<script>` element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineText {
    JavaScript,
    Data,
    /// The script has `src`, or is a classic script with `nomodule`, which iOS Safari skips.
    Ignored,
}

const SCRIPT: &[u8] = b"script";
const STYLE: &[u8] = b"style";
const LINK: &[u8] = b"link";

const SRC: &[u8] = b"src";
const SRCSET: &[u8] = b"srcset";
const IMAGE_SRCSET: &[u8] = b"imagesrcset";
const POSTER: &[u8] = b"poster";
const DATA: &[u8] = b"data";
const HREF: &[u8] = b"href";
const XLINK_HREF: &[u8] = b"xlink:href";
const REL: &[u8] = b"rel";
const TYPE: &[u8] = b"type";
const LANGUAGE: &[u8] = b"language";
const NO_MODULE: &[u8] = b"nomodule";
const STYLE_ATTRIBUTE: &[u8] = b"style";

/// Attributes whose whole value names a file, on any element.
const REFERENCE_ATTRIBUTES: &[&[u8]] = &[SRC, POSTER, DATA];

/// SVG elements whose `href` or `xlink:href` loads an image or a file. The tokenizer lowercases
/// `feImage`.
const SVG_HREF_ELEMENTS: &[&[u8]] = &[b"image", b"use", b"feimage"];

/// `<link rel>` values that make the browser fetch or connect to `href`.
const LINK_LOAD_RELS: &[&str] = &[
    "apple-touch-icon",
    "apple-touch-icon-precomposed",
    "dns-prefetch",
    "icon",
    "manifest",
    "mask-icon",
    "modulepreload",
    "preconnect",
    "prefetch",
    "preload",
    "stylesheet",
];

const MODULE_TYPE: &str = "module";

/// The JavaScript MIME types of the HTML standard. A browser runs a script whose type is one
/// of these, empty, or `module`.
const JAVASCRIPT_TYPES: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// The type a `language` attribute names is this prefix and its value.
const LANGUAGE_TYPE_PREFIX: &str = "text/";

/// Reads one HTML file.
pub fn read_page(html: &str) -> Page {
    let mut emitter = DefaultEmitter::<usize>::new_with_span();
    // Without this, markup inside a `<script>` is read as tags.
    emitter.naively_switch_states(true);
    let mut page = Page::default();
    let mut open_script: Option<OpenScript> = None;
    let mut open_style: Option<usize> = None;
    for token in Tokenizer::new_with_emitter(html, emitter) {
        let Ok(token) = token;
        match token {
            Token::StartTag(tag) => {
                page.read_attributes(&tag);
                match tag.name.as_slice() {
                    SCRIPT => open_script = Some(OpenScript::new(&tag)),
                    STYLE => open_style = Some(tag.span.end),
                    _ => {}
                }
            }
            Token::EndTag(tag) => match tag.name.as_slice() {
                SCRIPT => page.close_script(open_script.take(), tag.span.start),
                STYLE => page.close_style(html, open_style.take(), tag.span.start),
                _ => {}
            },
            _ => {}
        }
    }
    // An element left open runs to the end of the file.
    page.close_script(open_script, html.len());
    page.close_style(html, open_style, html.len());
    page
}

impl Page {
    fn read_attributes(&mut self, tag: &StartTag<usize>) {
        for name in REFERENCE_ATTRIBUTES {
            self.references.extend(attribute(tag, name));
        }
        if let Some(srcset) = attribute(tag, SRCSET) {
            self.references.extend(srcset_urls(&srcset));
        }
        let element = tag.name.as_slice();
        if element == LINK && link_loads(tag) {
            self.references.extend(attribute(tag, HREF));
            if let Some(srcset) = attribute(tag, IMAGE_SRCSET) {
                self.references.extend(srcset_urls(&srcset));
            }
        }
        if SVG_HREF_ELEMENTS.contains(&element) {
            self.references.extend(attribute(tag, HREF));
            self.references.extend(attribute(tag, XLINK_HREF));
        }
        self.styles.extend(attribute(tag, STYLE_ATTRIBUTE));
    }
}

impl OpenScript {
    fn new(tag: &StartTag<usize>) -> OpenScript {
        let kind = if attribute(tag, SRC).is_some() {
            InlineText::Ignored
        } else {
            inline_text(tag)
        };
        OpenScript {
            text_start: tag.span.end,
            kind,
        }
    }
}

impl Page {
    fn close_script(&mut self, script: Option<OpenScript>, text_end: usize) {
        let Some(script) = script else {
            return;
        };
        let text = script.text_start..text_end;
        match script.kind {
            InlineText::JavaScript => self.scripts.push(text),
            InlineText::Data => self.data_blocks.push(text),
            InlineText::Ignored => {}
        }
    }

    fn close_style(&mut self, html: &str, text_start: Option<usize>, text_end: usize) {
        let text = text_start.and_then(|text_start| html.get(text_start..text_end));
        self.styles.extend(text.map(str::to_owned));
    }
}

/// An attribute's value, with entities decoded.
fn attribute(tag: &StartTag<usize>, name: &[u8]) -> Option<String> {
    let value = tag.attributes.get(name)?;
    Some(String::from_utf8_lossy(&value.value).into_owned())
}

/// The URL of each candidate in a `srcset`, split as the HTML standard does: a URL runs to the
/// next whitespace, so it may hold commas, and its descriptors run to the next comma. The
/// descriptors are skipped, not checked.
fn srcset_urls(srcset: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
        if rest.is_empty() {
            return urls;
        }
        let url_end = rest
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let (candidate, after) = rest.split_at(url_end);
        let url = candidate.trim_end_matches(',');
        urls.push(url.to_owned());
        // A URL that ends in a comma has no descriptors.
        let has_descriptors = url.len() == candidate.len();
        rest = if has_descriptors {
            after.find(',').map_or("", |comma| &after[comma..])
        } else {
            after
        };
    }
}

/// Whether a `<link>` has a `rel` that loads its `href`.
fn link_loads(tag: &StartTag<usize>) -> bool {
    attribute(tag, REL).is_some_and(|rel| {
        rel.to_ascii_lowercase()
            .split_ascii_whitespace()
            .any(|token| LINK_LOAD_RELS.contains(&token))
    })
}

/// What a browser does with the text of a `<script>` without `src`, by the HTML standard's
/// rules for `type`, `language` and `nomodule`.
fn inline_text(tag: &StartTag<usize>) -> InlineText {
    let type_text = match (attribute(tag, TYPE), attribute(tag, LANGUAGE)) {
        (Some(kind), _) => kind
            .trim_matches(|c: char| c.is_ascii_whitespace())
            .to_ascii_lowercase(),
        (None, Some(language)) if !language.is_empty() => {
            format!("{LANGUAGE_TYPE_PREFIX}{}", language.to_ascii_lowercase())
        }
        _ => String::new(),
    };
    let is_classic = type_text.is_empty() || JAVASCRIPT_TYPES.contains(&type_text.as_str());
    if type_text == MODULE_TYPE {
        InlineText::JavaScript
    } else if !is_classic {
        InlineText::Data
    } else if attribute(tag, NO_MODULE).is_some() {
        InlineText::Ignored
    } else {
        InlineText::JavaScript
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_scripts(html: &str) -> Vec<&str> {
        read_page(html)
            .scripts
            .into_iter()
            .map(|range| &html[range])
            .collect()
    }

    fn data_blocks(html: &str) -> Vec<&str> {
        read_page(html)
            .data_blocks
            .into_iter()
            .map(|range| &html[range])
            .collect()
    }

    fn references(html: &str) -> Vec<String> {
        read_page(html).references
    }

    #[test]
    fn inline_scripts_are_byte_ranges_of_the_file() {
        let html = "<p>é</p><script>let a = 1;</script>";
        assert_eq!(inline_scripts(html), ["let a = 1;"]);
    }

    #[test]
    fn a_module_script_is_inline_javascript() {
        assert_eq!(inline_scripts("<script type=module>b()</script>"), ["b()"]);
    }

    #[test]
    fn a_legacy_javascript_type_is_inline_javascript() {
        let html = "<script type=text/ecmascript>run()</script>";
        assert_eq!(inline_scripts(html), ["run()"]);
    }

    #[test]
    fn the_type_is_read_without_case_or_surrounding_whitespace() {
        let html = "<script type=' Text/JavaScript '>run()</script>";
        assert_eq!(inline_scripts(html), ["run()"]);
    }

    #[test]
    fn a_script_with_src_is_not_inline_javascript() {
        assert!(inline_scripts("<script src=main.js>ignored()</script>").is_empty());
    }

    #[test]
    fn a_script_with_a_data_type_is_a_data_block() {
        let html = concat!(
            "<script type=application/json>{\"a\": 1}</script>",
            "<script type=importmap>{}</script>",
        );
        assert!(inline_scripts(html).is_empty());
        assert_eq!(data_blocks(html), ["{\"a\": 1}", "{}"]);
    }

    #[test]
    fn a_classic_script_with_nomodule_is_not_inline_javascript() {
        let html = "<script nomodule>legacy()</script>";
        assert!(inline_scripts(html).is_empty());
        assert!(data_blocks(html).is_empty());
    }

    #[test]
    fn a_module_script_with_nomodule_is_inline_javascript() {
        let html = "<script type=module nomodule>b()</script>";
        assert_eq!(inline_scripts(html), ["b()"]);
    }

    #[test]
    fn a_javascript_language_without_a_type_is_inline_javascript() {
        let html = "<script language=JavaScript>run()</script>";
        assert_eq!(inline_scripts(html), ["run()"]);
    }

    #[test]
    fn another_language_without_a_type_is_a_data_block() {
        let html = "<script language=vbscript>run()</script>";
        assert!(inline_scripts(html).is_empty());
        assert_eq!(data_blocks(html), ["run()"]);
    }

    #[test]
    fn the_language_is_not_read_when_there_is_a_type() {
        let html = "<script type=text/javascript language=vbscript>run()</script>";
        assert_eq!(inline_scripts(html), ["run()"]);
        assert!(data_blocks(html).is_empty());
    }

    #[test]
    fn markup_inside_a_script_stays_script_text() {
        let html = "<script>if (a<b) document.write('<p>x</p>');</script>";
        assert_eq!(
            inline_scripts(html),
            ["if (a<b) document.write('<p>x</p>');"]
        );
    }

    #[test]
    fn a_style_tag_inside_a_script_is_not_css() {
        let html = "<script>document.write('<style>x</style>');</script>";
        assert!(read_page(html).styles.is_empty());
    }

    #[test]
    fn an_unclosed_script_runs_to_the_end_of_the_file() {
        assert_eq!(inline_scripts("<script>start()</p>"), ["start()</p>"]);
    }

    #[test]
    fn reference_attributes_are_collected_from_any_element() {
        let html = concat!(
            "<script src=\"./main.js\"></script>",
            "<video poster=\"p.png\" src=\"v.mp4\"></video>",
            "<object data=\"o.svg\"></object>",
        );
        assert_eq!(references(html), ["./main.js", "v.mp4", "p.png", "o.svg"]);
    }

    #[test]
    fn attribute_values_have_entities_decoded() {
        assert_eq!(
            references("<img src=\"q.png?a=1&amp;b=2\">"),
            ["q.png?a=1&b=2"]
        );
    }

    #[test]
    fn each_srcset_candidate_is_a_reference() {
        assert_eq!(
            references("<img srcset=\"a.png 1x, b.png 2x,c.png\">"),
            ["a.png", "b.png", "c.png"]
        );
    }

    #[test]
    fn a_srcset_url_may_hold_commas() {
        assert_eq!(
            references("<img srcset=\"https://x.invalid/w_400,h_300/a.png 1x,b.png, c.png\">"),
            ["https://x.invalid/w_400,h_300/a.png", "b.png", "c.png"]
        );
    }

    #[test]
    fn a_link_that_loads_is_a_reference() {
        let html = concat!(
            "<link rel=preconnect href=\"https://fonts.example.invalid\">",
            "<link rel=\"shortcut icon\" href=\"favicon.ico\">",
        );
        assert_eq!(
            references(html),
            ["https://fonts.example.invalid", "favicon.ico"]
        );
    }

    #[test]
    fn an_icon_link_for_ios_or_safari_is_a_reference() {
        let html = concat!(
            "<link rel=apple-touch-icon-precomposed href=\"touch.png\">",
            "<link rel=mask-icon href=\"mask.svg\">",
        );
        assert_eq!(references(html), ["touch.png", "mask.svg"]);
    }

    #[test]
    fn each_imagesrcset_candidate_on_a_link_that_loads_is_a_reference() {
        let html = "<link rel=preload as=image href=a.png imagesrcset=\"a.png 1x, b.png 2x\">";
        assert_eq!(references(html), ["a.png", "a.png", "b.png"]);
    }

    #[test]
    fn an_svg_image_or_use_href_is_a_reference() {
        let html = "<svg><image href=\"a.png\"/><use xlink:href=\"s.svg#i\"/></svg>";
        assert_eq!(references(html), ["a.png", "s.svg#i"]);
    }

    #[test]
    fn a_link_or_anchor_that_loads_nothing_is_not_a_reference() {
        let html = concat!(
            "<link rel=canonical href=\"https://game.example.invalid\">",
            "<a href=\"https://elsewhere.example.invalid\">x</a>",
        );
        assert!(references(html).is_empty());
    }

    #[test]
    fn style_elements_and_attributes_are_css() {
        let html = "<style>a{b:url(x.png)}</style><div style=\"c:url(y.png)\"></div>";
        assert_eq!(read_page(html).styles, ["a{b:url(x.png)}", "c:url(y.png)"]);
    }
}
