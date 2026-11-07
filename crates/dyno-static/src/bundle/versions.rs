//! Engine versions: Pixi, Howler and web-sdk, and the engine textures need for mipmaps (design
//! section 8.5).

use std::collections::BTreeSet;

use dyno_schema::ids::MeasureId;
use dyno_schema::library::{Library, LibraryBucket};
use dyno_schema::measure::{Evidence, Measure, MeasureItem, MeasureValue, Method};
use dyno_schema::path::RelPath;
use oxc_ast::ast::TemplateLiteral;

use crate::bundle::eval::{Evaluator, Value};
use crate::bundle::{estimated, exact};
use crate::model::{Engine, saturating_i64};

/// What one script says about engines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineMarkers {
    /// The first Pixi console banner in the script.
    pub pixi_banner: Option<PixiBanner>,
    /// From a comment that names `pixi.js`, kept only when the build kept comments.
    pub pixi_comment_version: Option<String>,
    /// From Howler's legal comment.
    pub howler_comment_version: Option<String>,
    /// A Howler string or property that only Howler has.
    pub howler_fingerprint: bool,
    /// The web-sdk RGS paths found as whole string literals.
    pub rgs_paths: BTreeSet<&'static str>,
    /// The string literal `rgs_url`.
    pub rgs_url_literal: bool,
    /// A `__sveltekit_` name read as a global, or as a property of any object.
    pub sveltekit_global: bool,
}

/// Pixi's `PixiJS <version> - <renderer> - <site>` console banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PixiBanner {
    /// None when the version is an expression the evaluator cannot read.
    pub version: Option<String>,
    /// From the version when it is read, else from the site in the banner: 7 links
    /// `https://pixijs.com`, 8 `http://www.pixijs.com/`.
    pub major: u8,
}

/// The engines of a whole build. None means not detected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineVersions {
    pub pixi: Option<Version>,
    pub howler: Option<Version>,
    /// web-sdk packages are versioned 0.0.0, so the bundle never holds a version.
    pub web_sdk: bool,
}

impl EngineVersions {
    /// Each library the build holds, in `Library::ALL` order.
    pub fn libraries(&self) -> impl Iterator<Item = Library> + '_ {
        Library::ALL
            .iter()
            .copied()
            .filter(|&library| self.version(library).is_some())
    }

    /// A detected library's version as the result shows it, or None when it is not detected.
    pub fn version(&self, library: Library) -> Option<&str> {
        match library {
            Library::Pixi => self.pixi.as_ref().map(Version::as_text),
            Library::Howler => self.howler.as_ref().map(Version::as_text),
            Library::WebSdk => self.web_sdk.then_some(UNSUPPORTED),
        }
    }

    /// Whether the build holds a library of this bucket.
    pub fn has_bucket(&self, bucket: LibraryBucket) -> bool {
        self.libraries().any(|library| library.bucket() == bucket)
    }
}

/// An engine's version, or `Unsupported` when the engine is there but its version is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Version {
    Known(String),
    Unsupported,
}

/// One Pixi copy: a version, a major, or both.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PixiCopy {
    version: Option<String>,
    major: Option<u8>,
}

const PIXI_BANNER_START: &str = "PixiJS ";

const PIXI_7_MAJOR: u8 = 7;
const PIXI_8_MAJOR: u8 = 8;

/// The end of the banner, and the major it belongs to.
const PIXI_BANNER_SITES: &[(&str, u8)] = &[
    (" - https://pixijs.com", PIXI_7_MAJOR),
    (" - http://www.pixijs.com/", PIXI_8_MAJOR),
];

const PIXI_PACKAGE: &str = "pixi.js";

const HOWLER_COMMENT_PREFIX: &str = "howler.js v";

/// A warning only Howler logs.
const HOWLER_FINGERPRINT_TEXT: &str =
    "HTML5 Audio pool exhausted, returning potentially locked audio object.";
/// A property only Howler has.
const HOWLER_FINGERPRINT_PROPERTY: &str = "html5PoolSize";

/// The RGS paths of a web-sdk build. A build is web-sdk when it has two of them, the `rgs_url`
/// literal and a SvelteKit global.
const RGS_PATHS: &[&str] = &[
    "/wallet/authenticate",
    "/wallet/play",
    "/wallet/end-round",
    "/bet/event",
];
const RGS_URL: &str = "rgs_url";
const MIN_RGS_PATHS: usize = 2;
const SVELTEKIT_GLOBAL_PREFIX: &str = "__sveltekit_";

const UNSUPPORTED: &str = "unsupported";
const ENGINES_EVIDENCE: &str = "engine banners, comments and strings in scanned files";

/// `major.minor.patch`.
const SEMVER_NUMBERS: usize = 3;

impl EngineMarkers {
    /// A string literal, or a template with no expressions.
    pub fn read_literal(&mut self, text: &str) {
        if let Some(path) = RGS_PATHS.iter().find(|path| **path == text) {
            self.rgs_paths.insert(path);
        }
        self.rgs_url_literal |= text == RGS_URL;
        self.howler_fingerprint |= text == HOWLER_FINGERPRINT_TEXT;
    }

    /// A template with expressions: Pixi's banner is one.
    pub fn read_template<'a>(
        &mut self,
        template: &TemplateLiteral<'a>,
        evaluator: &Evaluator<'_, 'a>,
    ) {
        if self.pixi_banner.is_none() {
            self.pixi_banner = pixi_banner(template, evaluator);
        }
    }

    pub fn read_comment(&mut self, comment: &str) {
        if self.howler_comment_version.is_none() {
            self.howler_comment_version = comment
                .split_once(HOWLER_COMMENT_PREFIX)
                .and_then(|(_, rest)| semver_at_start(rest))
                .map(str::to_owned);
        }
        if self.pixi_comment_version.is_none() {
            // Only on the line that names Pixi: a license block lists other packages' versions.
            self.pixi_comment_version = comment
                .split_once(PIXI_PACKAGE)
                .and_then(|(_, rest)| first_semver(rest.lines().next()?))
                .map(str::to_owned);
        }
    }

    /// A property read with `.name`.
    pub fn read_property_name(&mut self, name: &str) {
        self.howler_fingerprint |= name == HOWLER_FINGERPRINT_PROPERTY;
        self.sveltekit_global |= name.starts_with(SVELTEKIT_GLOBAL_PREFIX);
    }

    /// An object literal's key. Unlike a property read, it is never a global.
    pub fn read_object_key(&mut self, name: &str) {
        self.howler_fingerprint |= name == HOWLER_FINGERPRINT_PROPERTY;
    }

    /// A global the script reads or writes without declaring it.
    pub fn read_global_name(&mut self, name: &str) {
        self.sveltekit_global |= name.starts_with(SVELTEKIT_GLOBAL_PREFIX);
    }
}

impl Version {
    pub fn as_text(&self) -> &str {
        match self {
            Version::Known(version) => version,
            Version::Unsupported => UNSUPPORTED,
        }
    }
}

/// The build's engines from each script's markers, and the engine textures use. `scripts` must
/// be sorted by file.
///
/// With two Pixi copies, the one in `render_file` (the worst render site's file) wins, else the
/// first.
pub fn engine_versions(
    scripts: &[(RelPath, EngineMarkers)],
    render_file: Option<&RelPath>,
) -> (EngineVersions, Engine) {
    let pixi_copies: Vec<(&RelPath, PixiCopy)> = scripts
        .iter()
        .filter_map(|(file, markers)| Some((file, markers.pixi_copy()?)))
        .collect();
    let pixi = pixi_copies
        .iter()
        .find(|(file, _)| Some(*file) == render_file)
        .or(pixi_copies.first())
        .map(|(_, copy)| copy.clone());
    let engine = match pixi.as_ref().and_then(|copy| copy.major) {
        Some(PIXI_7_MAJOR) => Engine::Pixi7,
        Some(PIXI_8_MAJOR) => Engine::Pixi8,
        _ => Engine::Unknown,
    };
    let versions = EngineVersions {
        pixi: pixi.map(|copy| copy.version.map_or(Version::Unsupported, Version::Known)),
        howler: howler_version(scripts),
        web_sdk: is_web_sdk(scripts),
    };
    (versions, engine)
}

/// `static.bundle.engines`: how many engines the build has, and each one's version, by engine
/// name.
pub fn engines_measure(
    versions: &EngineVersions,
    scanned_files: usize,
    not_covered: &[String],
) -> Measure {
    // Each library keyed `<bucket>.<library>`, so the result says what it does.
    let items: Vec<MeasureItem> = versions
        .libraries()
        .filter_map(|library| {
            let version = versions.version(library)?;
            Some(text_item(&library.item_key(), version))
        })
        .collect();
    let engines = MeasureValue::Integer(saturating_i64(items.len()));
    let mut measure = estimated(MeasureId::StaticBundleEngines, engines, items, exact());
    measure.evidence = vec![Evidence {
        method: Method::BundleScan,
        detail: format!("{ENGINES_EVIDENCE}: {scanned_files}"),
    }];
    measure.scope.not_covered = not_covered.to_vec();
    measure
}

/// The Pixi banner `template` is, with the major from its site and the version from its text
/// (Pixi 7) or its first expression (Pixi 8). None when `template` is not a banner.
fn pixi_banner<'a>(
    template: &TemplateLiteral<'a>,
    evaluator: &Evaluator<'_, 'a>,
) -> Option<PixiBanner> {
    let first = template.quasis.first()?.value.cooked?.as_str()?;
    let last = template.quasis.last()?.value.cooked?.as_str()?;
    let after_name = first.strip_prefix(PIXI_BANNER_START)?;
    let site_major = PIXI_BANNER_SITES
        .iter()
        .find(|(site, _)| last.ends_with(site))
        .map(|(_, major)| *major)?;
    let version = if after_name.is_empty() {
        let value = template
            .expressions
            .first()
            .and_then(|expression| evaluator.value(expression));
        value
            .as_ref()
            .and_then(Value::as_text)
            .filter(|text| semver_at_start(text) == Some(*text))
            .map(str::to_owned)
    } else {
        semver_at_start(after_name).map(str::to_owned)
    };
    let major = version.as_deref().and_then(major_of).unwrap_or(site_major);
    Some(PixiBanner { version, major })
}

/// `major.minor.patch` with an optional `-prerelease`, at the start of `text`.
fn semver_at_start(text: &str) -> Option<&str> {
    let mut end = 0;
    for part in 0..SEMVER_NUMBERS {
        if part > 0 {
            if !text[end..].starts_with('.') {
                return None;
            }
            end += 1;
        }
        let digits = text[end..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        end += digits;
    }
    let has_fourth_number = text[end..]
        .strip_prefix('.')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if has_fourth_number {
        return None;
    }
    if let Some(prerelease) = text[end..].strip_prefix('-') {
        let run = prerelease
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            .count();
        // A prerelease's dot-separated parts are never empty, so a dot cannot end it.
        let length = prerelease[..run].trim_end_matches('.').len();
        if length > 0 {
            end += 1 + length;
        }
    }
    Some(&text[..end])
}

/// The first semver in `text` that does not start inside a longer number.
fn first_semver(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let follows_a_number = index > 0 && matches!(bytes[index - 1], b'0'..=b'9' | b'.');
        if !byte.is_ascii_digit() || follows_a_number {
            continue;
        }
        if let Some(version) = semver_at_start(&text[index..]) {
            return Some(version);
        }
    }
    None
}

impl EngineMarkers {
    /// The script's Pixi copy from its banner and its comment. None when it has neither.
    fn pixi_copy(&self) -> Option<PixiCopy> {
        let banner = self.pixi_banner.as_ref();
        let banner_major = banner.map(|banner| banner.major);
        // The banner is the surer sign of the major. A comment that disagrees names another
        // package's version.
        let comment_version = self
            .pixi_comment_version
            .clone()
            .filter(|version| banner_major.is_none_or(|major| major_of(version) == Some(major)));
        let version = banner
            .and_then(|banner| banner.version.clone())
            .or(comment_version);
        let major = banner_major.or_else(|| version.as_deref().and_then(major_of));
        (version.is_some() || major.is_some()).then_some(PixiCopy { version, major })
    }
}

fn major_of(version: &str) -> Option<u8> {
    version.split('.').next()?.parse().ok()
}

/// The first Howler version in a legal comment, else `Unsupported` when a fingerprint shows
/// Howler is there.
fn howler_version(scripts: &[(RelPath, EngineMarkers)]) -> Option<Version> {
    let known = scripts
        .iter()
        .find_map(|(_, markers)| markers.howler_comment_version.clone());
    if let Some(version) = known {
        return Some(Version::Known(version));
    }
    let has_fingerprint = scripts
        .iter()
        .any(|(_, markers)| markers.howler_fingerprint);
    has_fingerprint.then_some(Version::Unsupported)
}

/// Two RGS paths, the `rgs_url` literal and a SvelteKit global, anywhere in the build.
fn is_web_sdk(scripts: &[(RelPath, EngineMarkers)]) -> bool {
    let rgs_paths: BTreeSet<&str> = scripts
        .iter()
        .flat_map(|(_, markers)| markers.rgs_paths.iter().copied())
        .collect();
    let has_rgs_url = scripts.iter().any(|(_, markers)| markers.rgs_url_literal);
    let has_sveltekit_global = scripts.iter().any(|(_, markers)| markers.sveltekit_global);
    rgs_paths.len() >= MIN_RGS_PATHS && has_rgs_url && has_sveltekit_global
}

fn text_item(key: &str, value: &str) -> MeasureItem {
    MeasureItem {
        key: key.to_owned(),
        value: MeasureValue::Text(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(raw: &str) -> RelPath {
        RelPath::new(raw).unwrap()
    }

    fn banner(version: Option<&str>, major: u8) -> EngineMarkers {
        EngineMarkers {
            pixi_banner: Some(PixiBanner {
                version: version.map(str::to_owned),
                major,
            }),
            ..EngineMarkers::default()
        }
    }

    fn two_pixi_copies() -> [(RelPath, EngineMarkers); 2] {
        [
            (file("a.js"), banner(Some("7.4.3"), 7)),
            (file("b.js"), banner(Some("8.22.0"), 8)),
        ]
    }

    fn rgs_markers() -> EngineMarkers {
        let mut markers = EngineMarkers::default();
        for text in ["/wallet/authenticate", "/wallet/play", "rgs_url"] {
            markers.read_literal(text);
        }
        markers
    }

    #[test]
    fn a_semver_is_read_without_trailing_text() {
        assert_eq!(semver_at_start("7.4.3 - x"), Some("7.4.3"));
    }

    #[test]
    fn a_semver_keeps_its_prerelease() {
        assert_eq!(semver_at_start("8.0.0-rc.2 "), Some("8.0.0-rc.2"));
    }

    #[test]
    fn a_semver_prerelease_does_not_end_in_a_dot() {
        assert_eq!(semver_at_start("1.0.0-rc. "), Some("1.0.0-rc"));
    }

    #[test]
    fn a_semver_has_exactly_three_numbers_at_the_start() {
        assert_eq!(semver_at_start("8.22"), None);
        assert_eq!(semver_at_start("v8.22.0"), None);
        assert_eq!(semver_at_start("2024.1.1.5"), None);
    }

    #[test]
    fn a_comment_semver_may_follow_a_v() {
        assert_eq!(first_semver(" - v7.4.3 * Compiled"), Some("7.4.3"));
    }

    #[test]
    fn a_comment_semver_does_not_start_inside_a_longer_number() {
        assert_eq!(first_semver("build 12.2.3.4 then 1.0.0"), Some("1.0.0"));
    }

    #[test]
    fn a_pixi_comment_version_is_on_the_line_that_names_pixi() {
        let mut markers = EngineMarkers::default();
        markers.read_comment("/*! pixi.js/lib/index.js\n * tiny-lru @version 11.4.7 */");
        assert_eq!(markers.pixi_comment_version, None);
    }

    #[test]
    fn howler_is_read_from_its_legal_comment() {
        let mut markers = EngineMarkers::default();
        markers.read_comment("/*!\n *  howler.js v2.2.4\n *  howlerjs.com\n */");
        assert_eq!(markers.howler_comment_version.as_deref(), Some("2.2.4"));
    }

    #[test]
    fn howler_without_a_comment_is_unsupported_when_a_fingerprint_is_there() {
        let mut markers = EngineMarkers::default();
        markers.read_property_name("html5PoolSize");
        let (versions, _) = engine_versions(&[(file("main.js"), markers)], None);
        assert_eq!(versions.howler, Some(Version::Unsupported));
    }

    #[test]
    fn a_pixi_comment_gives_the_version_when_the_banner_has_none() {
        let mut markers = banner(None, 8);
        markers.read_comment("/*! pixi.js - v8.1.0 */");
        let (versions, engine) = engine_versions(&[(file("main.js"), markers)], None);
        assert_eq!(versions.pixi, Some(Version::Known("8.1.0".into())));
        assert_eq!(engine, Engine::Pixi8);
    }

    #[test]
    fn a_pixi_comment_with_another_major_than_the_banner_is_ignored() {
        let mut markers = banner(None, 8);
        markers.read_comment("/*! pixi.js plugin, peer 7.4.3 */");
        let (versions, engine) = engine_versions(&[(file("main.js"), markers)], None);
        assert_eq!(versions.pixi, Some(Version::Unsupported));
        assert_eq!(engine, Engine::Pixi8);
    }

    #[test]
    fn a_banner_without_a_version_gives_the_major_only() {
        let (versions, engine) = engine_versions(&[(file("main.js"), banner(None, 7))], None);
        assert_eq!(versions.pixi, Some(Version::Unsupported));
        assert_eq!(engine, Engine::Pixi7);
    }

    #[test]
    fn with_two_pixi_copies_the_render_site_file_wins() {
        let (versions, engine) = engine_versions(&two_pixi_copies(), Some(&file("b.js")));
        assert_eq!(versions.pixi, Some(Version::Known("8.22.0".into())));
        assert_eq!(engine, Engine::Pixi8);
    }

    #[test]
    fn with_two_pixi_copies_and_no_render_site_the_first_wins() {
        let (_, engine) = engine_versions(&two_pixi_copies(), None);
        assert_eq!(engine, Engine::Pixi7);
    }

    #[test]
    fn web_sdk_needs_a_sveltekit_global() {
        let scripts = [(file("main.js"), rgs_markers())];
        assert!(!engine_versions(&scripts, None).0.web_sdk);
    }

    #[test]
    fn web_sdk_markers_may_be_in_different_files() {
        let mut inline = EngineMarkers::default();
        inline.read_global_name("__sveltekit_abc");
        let scripts = [
            (file("index.html"), inline),
            (file("main.js"), rgs_markers()),
        ];
        assert!(engine_versions(&scripts, None).0.web_sdk);
    }

    #[test]
    fn one_rgs_path_is_not_web_sdk() {
        let mut markers = EngineMarkers::default();
        for text in ["/wallet/play", "/wallet/play", "rgs_url"] {
            markers.read_literal(text);
        }
        markers.read_global_name("__sveltekit_abc");
        assert!(
            !engine_versions(&[(file("main.js"), markers)], None)
                .0
                .web_sdk
        );
    }

    #[test]
    fn the_engines_measure_counts_each_library_with_its_version_keyed_by_bucket() {
        let versions = EngineVersions {
            pixi: Some(Version::Known("8.22.0".into())),
            howler: Some(Version::Unsupported),
            web_sdk: true,
        };
        let measure = engines_measure(&versions, 1, &[]);
        assert_eq!(measure.value, Some(MeasureValue::Integer(3)));
        assert_eq!(
            measure.items,
            [
                text_item("audio.howler", "unsupported"),
                text_item("rendering.pixi", "8.22.0"),
                text_item("other.web_sdk", "unsupported"),
            ]
        );
    }
}
