//! Spine atlas files: which page images get mips, and which pages are missing from the build
//! (design section 7.5).
//!
//! An atlas is text. Pages are separated by blank lines. A page's first line is its image file,
//! relative to the atlas, then its `key: value` fields. The first line without a colon starts its
//! regions, which this reader skips. Spine 3.x and 4.x share this layout, and 4.x may put header
//! fields before the first page.

use std::collections::BTreeSet;

use dyno_schema::path::RelPath;

use crate::model::Inventory;

/// One page of a Spine atlas.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AtlasPage {
    /// The image file as the atlas writes it, relative to the atlas.
    image: String,
    /// The first value of the page's `filter` field, if it has one.
    min_filter: Option<String>,
}

/// What the build's atlases say about its images.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpineLinks {
    /// Page images whose min filter is a MipMap filter.
    pub mip_pages: BTreeSet<RelPath>,
    /// Pages whose image is not in the build, sorted by atlas, once each.
    pub missing_pages: BTreeSet<MissingPage>,
}

/// A page whose image is not in the build.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MissingPage {
    /// The atlas that names the page.
    pub atlas: RelPath,
    /// The image file as the atlas writes it, relative to the atlas.
    pub image: String,
}

/// Where the page reader is in an atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// After a blank line, or at the start: the next line names a page.
    BetweenPages,
    /// The `key: value` lines after a page's name.
    PageFields,
    /// The page's regions, up to the next blank line.
    Regions,
}

/// Spine exports `.atlas`, and some pipelines name the same file `.atlas.txt`.
const ATLAS_ENDINGS: [&str; 2] = [".atlas", ".atlas.txt"];
const FILTER_KEY: &str = "filter";
/// Spine's mip filters are MipMap, MipMapNearestNearest, MipMapLinearNearest and so on.
const MIPMAP_FILTER_PREFIX: &str = "MipMap";
const BYTE_ORDER_MARK: char = '\u{feff}';

impl AtlasPage {
    /// Whether the runtime makes mips for this page.
    fn has_mipmap_filter(&self) -> bool {
        self.min_filter
            .as_deref()
            .is_some_and(|filter| filter.starts_with(MIPMAP_FILTER_PREFIX))
    }
}

/// Reads every atlas in the build and links its pages to the build's images.
pub fn link(inventory: &Inventory) -> SpineLinks {
    let mut links = SpineLinks::default();
    for file in inventory.files().iter().filter(|file| is_atlas(&file.path)) {
        let atlas = &file.path;
        // Never None, since the inventory holds every file it lists.
        let text = String::from_utf8_lossy(inventory.contents(atlas).unwrap_or_default());
        for page in pages(&text) {
            let image_in_build =
                page_path(atlas, &page.image).filter(|path| inventory.contents(path).is_some());
            let Some(image_path) = image_in_build else {
                links.missing_pages.insert(MissingPage {
                    atlas: atlas.clone(),
                    image: page.image,
                });
                continue;
            };
            if page.has_mipmap_filter() {
                links.mip_pages.insert(image_path);
            }
        }
    }
    links
}

/// Every page of an atlas, in file order.
fn pages(text: &str) -> Vec<AtlasPage> {
    let mut atlas_pages: Vec<AtlasPage> = Vec::new();
    let mut section = Section::BetweenPages;
    for line in lines(text) {
        if line.is_empty() {
            section = Section::BetweenPages;
            continue;
        }
        let field = line.split_once(':');
        match section {
            Section::BetweenPages => {
                let is_header_field = atlas_pages.is_empty() && field.is_some();
                if !is_header_field {
                    atlas_pages.push(AtlasPage {
                        image: line.to_owned(),
                        min_filter: None,
                    });
                    section = Section::PageFields;
                }
            }
            Section::PageFields => {
                let Some((key, values)) = field else {
                    // A line without a colon is the first region's name.
                    section = Section::Regions;
                    continue;
                };
                // There is always a page here: the line that started PageFields added it.
                if key.trim() == FILTER_KEY
                    && let Some(page) = atlas_pages.last_mut()
                {
                    page.min_filter = Some(min_filter(values).to_owned());
                }
            }
            Section::Regions => {}
        }
    }
    atlas_pages
}

/// The atlas's lines, trimmed. Spine ends a line at `\r\n`, `\n` or a lone `\r`.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.trim_start_matches(BYTE_ORDER_MARK)
        .lines()
        .flat_map(|line| line.split('\r'))
        .map(str::trim)
}

/// The min filter from `filter: <min>, <mag>`. Only the min filter decides mips.
fn min_filter(values: &str) -> &str {
    values.split_once(',').map_or(values, |(min, _)| min).trim()
}

fn is_atlas(path: &RelPath) -> bool {
    let path = path.as_str().to_ascii_lowercase();
    ATLAS_ENDINGS.iter().any(|ending| path.ends_with(ending))
}

/// The page image's path in the build: `image` resolved against the atlas's folder. None when it
/// climbs out of the build.
fn page_path(atlas: &RelPath, image: &str) -> Option<RelPath> {
    let mut parts: Vec<&str> = atlas.as_str().split('/').collect();
    // Drop the atlas's file name to get its folder.
    parts.pop();
    // An atlas written on Windows may use backslashes.
    for part in image.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    RelPath::new(&parts.join("/")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPINE_4_ATLAS: &str = "\
hero.png
size: 1024, 1024
filter: Linear, Linear
pma: true
body
bounds: 0, 0, 512, 512

hero_2.png
size: 512, 256
filter: MipMapLinearLinear, Linear
cape
bounds: 0, 0, 512, 256
";

    // It starts with a blank CRLF line.
    const SPINE_3_ATLAS: &str = "\r
sheet.png\r
size: 256,256\r
format: RGBA8888\r
filter: MipMapNearestNearest,Nearest\r
repeat: none\r
coin\r
  rotate: false\r
  xy: 2, 2\r
";

    fn path(raw: &str) -> RelPath {
        RelPath::new(raw).unwrap()
    }

    fn page(image: &str, min_filter: &str) -> AtlasPage {
        AtlasPage {
            image: image.to_owned(),
            min_filter: Some(min_filter.to_owned()),
        }
    }

    #[test]
    fn a_spine_4_atlas_gives_each_page_and_its_min_filter() {
        assert_eq!(
            pages(SPINE_4_ATLAS),
            [
                page("hero.png", "Linear"),
                page("hero_2.png", "MipMapLinearLinear")
            ]
        );
    }

    #[test]
    fn a_spine_3_atlas_with_crlf_and_a_leading_blank_line_gives_its_page_and_min_filter() {
        assert_eq!(
            pages(SPINE_3_ATLAS),
            [page("sheet.png", "MipMapNearestNearest")]
        );
    }

    #[test]
    fn a_lone_carriage_return_ends_a_line() {
        let atlas = "hero.png\rsize: 64,64\rfilter: MipMap,Linear\r\rhero_2.png\r";
        assert_eq!(
            pages(atlas),
            [
                page("hero.png", "MipMap"),
                AtlasPage {
                    image: "hero_2.png".to_owned(),
                    min_filter: None,
                }
            ]
        );
    }

    #[test]
    fn header_fields_before_the_first_page_are_not_pages() {
        let atlas = "version: 4.2\nhero.png\nfilter: Linear,Linear\n";
        assert_eq!(pages(atlas), [page("hero.png", "Linear")]);
    }

    #[test]
    fn a_filter_line_in_a_region_does_not_change_the_page() {
        let atlas = "hero.png\nfilter: Linear,Linear\nbody\nfilter: MipMap,MipMap\n";
        assert_eq!(pages(atlas), [page("hero.png", "Linear")]);
    }

    #[test]
    fn a_line_of_spaces_ends_a_page_and_spaces_around_a_name_are_dropped() {
        let atlas = "  hero.png  \nfilter: Linear,Linear\n   \nhero_2.png\t\n";
        assert_eq!(
            pages(atlas),
            [
                page("hero.png", "Linear"),
                AtlasPage {
                    image: "hero_2.png".to_owned(),
                    min_filter: None,
                }
            ]
        );
    }

    #[test]
    fn a_byte_order_mark_is_not_part_of_the_first_image_name() {
        assert_eq!(
            pages("\u{feff}hero.png\nfilter: Linear,Linear\n"),
            [page("hero.png", "Linear")]
        );
    }

    #[test]
    fn a_page_without_a_filter_has_no_mips() {
        let no_filter = AtlasPage {
            image: "hero.png".to_owned(),
            min_filter: None,
        };
        assert!(!no_filter.has_mipmap_filter());
        assert_eq!(pages("hero.png\nsize: 64,64\n"), [no_filter]);
    }

    #[test]
    fn only_a_mipmap_min_filter_makes_mips() {
        assert!(page("a.png", "MipMap").has_mipmap_filter());
        assert!(page("a.png", "MipMapLinearNearest").has_mipmap_filter());
        assert!(!page("a.png", "Linear").has_mipmap_filter());
        assert!(!page("a.png", "Nearest").has_mipmap_filter());
    }

    #[test]
    fn an_atlas_ends_in_atlas_or_atlas_txt_in_any_case() {
        assert!(is_atlas(&path("spine/hero.atlas")));
        assert!(is_atlas(&path("spine/HERO.ATLAS")));
        assert!(is_atlas(&path("spine/hero.atlas.txt")));
        assert!(!is_atlas(&path("spine/hero.json")));
    }

    #[test]
    fn a_page_resolves_against_the_atlas_folder() {
        let atlas = path("assets/spine/hero.atlas");
        assert_eq!(
            page_path(&atlas, "hero.png"),
            Some(path("assets/spine/hero.png"))
        );
        assert_eq!(
            page_path(&atlas, "./pages/hero.png"),
            Some(path("assets/spine/pages/hero.png"))
        );
        assert_eq!(
            page_path(&atlas, "../shared/hero.png"),
            Some(path("assets/shared/hero.png"))
        );
    }

    #[test]
    fn a_page_path_may_use_backslashes() {
        assert_eq!(
            page_path(&path("assets/spine/hero.atlas"), "..\\shared\\hero.png"),
            Some(path("assets/shared/hero.png"))
        );
    }

    #[test]
    fn a_dot_or_an_empty_part_is_not_a_folder_to_climb_out_of() {
        let atlas = path("assets/spine/hero.atlas");
        assert_eq!(
            page_path(&atlas, "./../shared/hero.png"),
            Some(path("assets/shared/hero.png"))
        );
        assert_eq!(
            page_path(&atlas, "pages//../hero.png"),
            Some(path("assets/spine/hero.png"))
        );
    }

    #[test]
    fn a_page_that_climbs_out_of_the_build_has_no_path() {
        assert_eq!(page_path(&path("hero.atlas"), "../hero.png"), None);
    }
}
