//! Bundle: what the HTML, JavaScript, CSS and JSON say about rendering, engines, external URLs
//! and Howl sites.
//!
//! Read the build, not the SDK: a game can ship `antialias: true` while its SDK defaults differ.
//! The canvas floor comes from the render config and the host profile's frame (design section 8).

use std::borrow::Cow;

use dyno_schema::ids::MeasureId;
use dyno_schema::library::LibraryBucket;
use dyno_schema::manifest::FileKind;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, NOT_BUILT_YET,
    Uncertainty, UncertaintyKind, missing_measures,
};
use dyno_schema::path::RelPath;
use dyno_schema::profile::HostProfile;

use crate::bundle::js::{ScriptAt, ScriptScan};
use crate::bundle::render::{DefaultWrites, RenderConfig};
use crate::bundle::urls::{ExternalUrls, UrlBook};
use crate::bundle::versions::{EngineMarkers, EngineVersions};
use crate::model::{BundleSummary, Engine, FamilyScan, HowlSite, Inventory, saturating_i64};
use crate::sniff::is_source_map;

pub mod css;
pub mod eval;
pub mod howl;
pub mod html;
pub mod js;
pub mod render;
pub mod requests;
pub mod urls;
pub mod versions;

/// Everything the bundle scan found, with the rows its fixture tests compare (design section
/// 8.8).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleReport {
    /// The files scanned for scripts, sorted: every JavaScript file, and every HTML file with an
    /// inline script.
    pub scanned: Vec<RelPath>,
    /// The scanned files with a script that did not parse, sorted.
    pub invalid: Vec<RelPath>,
    pub versions: EngineVersions,
    /// The engine whose rules decide which textures get mipmaps.
    pub engine: Engine,
    /// Sorted by file, then offset.
    pub render_configs: Vec<RenderConfig>,
    /// Sorted by file, then offset.
    pub howl_sites: Vec<HowlSite>,
    pub external: ExternalUrls,
    /// Whether a page has a canvas element, or a script may use a canvas.
    pub uses_a_canvas: bool,
    /// Whether the build has an HTML page.
    pub has_a_page: bool,
}

/// Collects what each file gives.
struct BuildReader<'p> {
    profile: &'p HostProfile,
    report: BundleReport,
    urls: UrlBook,
    /// Each script's engine markers. An HTML file has one entry per inline script.
    markers: Vec<(RelPath, EngineMarkers)>,
    /// The first write of each Pixi default in the build.
    default_writes: DefaultWrites,
}

/// The measures this family owns. The source map breakdown is Phase 2.
pub const MEASURES: &[MeasureId] = &[
    MeasureId::StaticRenderAntialias,
    MeasureId::StaticRenderResolutionUncapped,
    MeasureId::StaticRenderBackend,
    MeasureId::StaticRenderCanvas,
    MeasureId::StaticBundleEngines,
    MeasureId::StaticBundleExternalUrls,
    MeasureId::StaticBundleSourceMap,
];

const SCRIPT_NOT_PARSED: &str = "a script that did not parse";

/// A canvas element in a page's markup.
const CANVAS_TAG: &str = "<canvas";
const HOSTS_BUILT_AT_RUN_TIME: &str = "hosts built at run time";
const EXTERNAL_URLS_EVIDENCE: &str =
    "whole URL values in HTML, CSS, JSON and the request positions of scripts";

/// Each external URL is one item that counts once.
const URL_ITEM_COUNT: u64 = 1;

/// A byte order mark, which `serde_json` does not skip.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// Parses the scripts for the render config, engine versions, external URLs and Howl sites,
/// and takes the canvas floor in the profile's frame.
pub fn scan(inventory: &Inventory, profile: &HostProfile) -> FamilyScan<BundleSummary> {
    // The stub scan passes an empty inventory and expects every measure missing.
    if *inventory == Inventory::default() {
        return FamilyScan {
            measures: missing_measures(MEASURES, NOT_BUILT_YET),
            summary: BundleSummary::default(),
        };
    }
    let report = analyse(inventory, profile);
    let not_covered: Vec<String> = report
        .invalid
        .iter()
        .map(|file| format!("{SCRIPT_NOT_PARSED}: {file}"))
        .collect();
    let draws_with_html = draws_with_html(&report);
    let mut measures = render::measures(&report.render_configs, draws_with_html, &not_covered);
    measures.push(versions::engines_measure(
        &report.versions,
        report.scanned.len(),
        &not_covered,
    ));
    measures.push(external_urls_measure(&report.external, &not_covered));
    measures.push(Measure::missing(
        MeasureId::StaticBundleSourceMap,
        NOT_BUILT_YET,
    ));
    let summary = BundleSummary {
        engine: report.engine,
        render_sites: report
            .render_configs
            .iter()
            .map(RenderConfig::site)
            .collect(),
        howl_sites: report.howl_sites,
        invalid_scripts: report.invalid,
        draws_with_html,
    };
    FamilyScan { measures, summary }
}

/// Reads every HTML, JavaScript, CSS and JSON file in the build, except source maps.
pub fn analyse(inventory: &Inventory, profile: &HostProfile) -> BundleReport {
    let mut reader = BuildReader {
        profile,
        report: BundleReport::default(),
        urls: UrlBook::default(),
        markers: Vec::new(),
        default_writes: DefaultWrites::default(),
    };
    for (file, text) in texts(inventory, FileKind::Html) {
        reader.read_html(file, &text);
    }
    for (file, text) in texts(inventory, FileKind::Js) {
        reader.read_js(file, &text);
    }
    for (file, text) in texts(inventory, FileKind::Css) {
        reader.read_css(file, &text);
    }
    for (file, text) in texts(inventory, FileKind::Data) {
        reader.read_json(file, &text);
    }
    reader.finish()
}

/// An estimated measure with a value.
pub(crate) fn estimated(
    id: MeasureId,
    value: MeasureValue,
    items: Vec<MeasureItem>,
    uncertainty: Uncertainty,
) -> Measure {
    Measure {
        value: Some(value),
        items,
        uncertainty,
        ..Measure::blank(id, MeasureStatus::Estimated)
    }
}

/// The uncertainty of a value the build states outright.
pub(crate) fn exact() -> Uncertainty {
    Uncertainty {
        kind: UncertaintyKind::Exact,
        ..Uncertainty::unknown()
    }
}

/// Each file of one kind with its text, source maps left out. Invalid UTF-8 is replaced with
/// U+FFFD, as a browser does.
fn texts(inventory: &Inventory, kind: FileKind) -> impl Iterator<Item = (&RelPath, Cow<'_, str>)> {
    // `Inventory::new` holds the contents of every listed file.
    inventory.files_of_kind(kind).filter_map(|file| {
        if is_source_map(&file.path) {
            return None;
        }
        let bytes = inventory.contents(&file.path)?;
        Some((&file.path, String::from_utf8_lossy(bytes)))
    })
}

impl BuildReader<'_> {
    fn read_html(&mut self, file: &RelPath, text: &str) {
        self.report.has_a_page = true;
        if text.to_ascii_lowercase().contains(CANVAS_TAG) {
            self.report.uses_a_canvas = true;
        }
        let page = html::read_page(text);
        for reference in &page.references {
            self.add_url(file, reference);
        }
        for style in &page.styles {
            self.read_css(file, style);
        }
        for data_block in &page.data_blocks {
            if let Some(data) = text.get(data_block.clone()) {
                self.read_json(file, data);
            }
        }
        for script in &page.scripts {
            match text.get(script.clone()) {
                Some(script_text) => self.read_script(file, script_text, script.start),
                None => self.report.invalid.push(file.clone()),
            }
        }
        if !page.scripts.is_empty() {
            self.report.scanned.push(file.clone());
        }
    }

    fn read_js(&mut self, file: &RelPath, text: &str) {
        self.read_script(file, text, 0);
        self.report.scanned.push(file.clone());
    }

    /// One script whose text starts at byte `offset` of `file`.
    fn read_script(&mut self, file: &RelPath, text: &str, offset: usize) {
        let Some(scan) = self.scan_script(file, text, offset) else {
            self.report.invalid.push(file.clone());
            return;
        };
        let ScriptScan {
            render_configs,
            howl_sites,
            urls,
            dynamic_host_sites,
            markers,
            default_writes,
            uses_a_canvas,
        } = scan;
        self.report.uses_a_canvas |= uses_a_canvas;
        self.report.render_configs.extend(render_configs);
        self.report.howl_sites.extend(howl_sites);
        for url in urls {
            self.urls.add(url, file);
        }
        self.urls.add_dynamic_host_sites(dynamic_host_sites);
        self.markers.push((file.clone(), markers));
        self.default_writes.add_all(default_writes);
    }

    /// None when the script does not parse, or ends past 4 GiB, since offsets are 32 bits.
    fn scan_script(&self, file: &RelPath, text: &str, offset: usize) -> Option<ScriptScan> {
        let end = offset.checked_add(text.len())?;
        u32::try_from(end).ok()?;
        let offset = u32::try_from(offset).ok()?;
        js::scan_script(ScriptAt { file, offset }, text, self.profile).ok()
    }

    fn read_css(&mut self, file: &RelPath, text: &str) {
        for reference in css::references(text) {
            self.add_url(file, &reference);
        }
    }

    /// Every string value that is a whole URL. A file that is not JSON gives nothing.
    fn read_json(&mut self, file: &RelPath, text: &str) {
        let text = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(text);
        let Ok(root) = serde_json::from_str::<serde_json::Value>(text) else {
            return;
        };
        let mut pending = vec![&root];
        while let Some(node) = pending.pop() {
            match node {
                serde_json::Value::String(string) => self.add_url(file, string),
                serde_json::Value::Array(items) => pending.extend(items),
                serde_json::Value::Object(fields) => pending.extend(fields.values()),
                _ => {}
            }
        }
    }

    fn add_url(&mut self, file: &RelPath, value: &str) {
        if let Some(url) = urls::whole_url(value) {
            self.urls.add(url, file);
        }
    }

    fn finish(self) -> BundleReport {
        let mut report = self.report;
        for config in &mut report.render_configs {
            config.follow_default_writes(&self.default_writes);
        }
        report.scanned.sort();
        report.invalid.sort();
        report.invalid.dedup();
        report
            .render_configs
            .sort_by(|a, b| (&a.file, a.offset).cmp(&(&b.file, b.offset)));
        report
            .howl_sites
            .sort_by(|a, b| (&a.file, a.offset).cmp(&(&b.file, b.offset)));
        let render_file = worst_render_file(&report.render_configs);
        let mut markers = self.markers;
        markers.sort_by(|a, b| a.0.cmp(&b.0));
        let (versions, engine) = versions::engine_versions(&markers, render_file);
        report.versions = versions;
        report.engine = engine;
        report.external = self.urls.finish();
        report
    }
}

/// Whether the build shows that the game draws with HTML and CSS alone: it has a page, and no
/// render config, no rendering library, no canvas element or script that may use a canvas, no
/// script that did not parse, and no external URL that could load one. A host built at run time
/// is not checked. The label is about the rendering surface, so of the libraries only the
/// rendering bucket counts (Pixi); audio (Howler) and other libraries (web-sdk) do not. A
/// renderer the scan does not know is caught by its canvas use.
fn draws_with_html(report: &BundleReport) -> bool {
    let has_a_rendering_engine = report.versions.has_bucket(LibraryBucket::Rendering);
    report.has_a_page
        && report.render_configs.is_empty()
        && !has_a_rendering_engine
        && !report.uses_a_canvas
        && report.invalid.is_empty()
        && report.external.urls.is_empty()
}

/// The file of the config with the largest canvas, else of the first config.
fn worst_render_file(configs: &[RenderConfig]) -> Option<&RelPath> {
    let worst = render::worst_config(configs)
        .map(|(config, _)| config)
        .or(configs.first());
    worst.map(|config| &config.file)
}

/// `static.bundle.external_urls`: the number of URLs, one item each.
fn external_urls_measure(external: &ExternalUrls, not_covered: &[String]) -> Measure {
    let items = external
        .urls
        .iter()
        .map(|url| MeasureItem {
            key: url.url.clone(),
            value: MeasureValue::Integer(saturating_i64(URL_ITEM_COUNT)),
        })
        .collect();
    let count = MeasureValue::Integer(saturating_i64(external.urls.len()));
    let mut measure = estimated(MeasureId::StaticBundleExternalUrls, count, items, exact());
    measure.evidence = vec![Evidence {
        method: Method::BundleScan,
        detail: format!("{EXTERNAL_URLS_EVIDENCE}, hosts: {}", external.hosts.len()),
    }];
    measure.scope.not_covered = not_covered.to_vec();
    if external.dynamic_host_sites > 0 {
        let sites = external.dynamic_host_sites;
        measure
            .scope
            .not_covered
            .push(format!("{HOSTS_BUILT_AT_RUN_TIME}: {sites}"));
    }
    measure
}
