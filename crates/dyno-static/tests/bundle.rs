//! The bundle scan against each fixture's `[bundle]` table, with its `[profile]` (design section
//! 8).

mod common;

use common::{Fixture, fixture, fixtures};
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue, NOT_BUILT_YET};
use dyno_schema::path::RelPath;
use dyno_schema::profile::HostProfile;
use dyno_static::bundle::render::RenderConfig;
use dyno_static::bundle::urls::ExternalUrls;
use dyno_static::bundle::versions::{EngineVersions, Version};
use dyno_static::bundle::{self, BundleReport, analyse};
use dyno_static::model::{Engine, Inventory, SpriteRange};
use toml::{Table, Value};

/// Not compared: a note for people (design section 8.8).
const RESOLUTION_NOTE: &str = "resolution_from";

#[test]
fn scanned_and_invalid_scripts_match_every_fixture() {
    for (fixture, report, expected) in folder_fixtures() {
        let name = &fixture.name;
        assert_eq!(paths(&report.scanned), expected["scanned"], "{name}");
        assert_eq!(paths(&report.invalid), expected["invalid"], "{name}");
    }
}

#[test]
fn engine_versions_match_every_fixture() {
    for (fixture, report, expected) in folder_fixtures() {
        assert_eq!(
            versions_table(&report.versions),
            expected["versions"],
            "{}",
            fixture.name
        );
    }
}

#[test]
fn render_sites_match_every_fixture() {
    for (fixture, report, expected) in folder_fixtures() {
        let mut expected_rows = expected
            .get("render_site")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        for row in expected_rows.as_array_mut().expect("render_site rows") {
            row.as_table_mut()
                .expect("a render_site row is a table")
                .remove(RESOLUTION_NOTE);
        }
        assert_eq!(
            render_rows(&report.render_configs),
            expected_rows,
            "{}",
            fixture.name
        );
    }
}

#[test]
fn drawing_with_html_matches_every_folder_fixture() {
    for fixture in fixtures().into_iter().filter(|fixture| !fixture.is_zip()) {
        let expected = fixture.required_table("bundle");
        let listed = expected
            .get("draws_with_html")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let scan = bundle::scan(&fixture.inventory(), &fixture.profile());
        assert_eq!(scan.summary.draws_with_html, listed, "{}", fixture.name);
    }
}

#[test]
fn external_urls_match_every_fixture() {
    for (fixture, report, expected) in folder_fixtures() {
        assert_eq!(
            external_table(&report.external),
            expected["external"],
            "{}",
            fixture.name
        );
    }
}

#[test]
fn a_literal_howl_site_gives_its_src_list_and_sprites_in_microseconds() {
    let report = report_of("howler-sprites");
    let sprite = |name: &str, start_us, end_us, looped| SpriteRange {
        name: name.to_owned(),
        start_us,
        end_us,
        looped,
    };
    assert_eq!(report.howl_sites.len(), 1);
    let site = &report.howl_sites[0];
    assert_eq!(site.file, path("main.js"));
    assert_eq!(site.src, ["assets/audio/sfx.webm", "assets/audio/sfx.mp3"]);
    assert_eq!(
        site.sprites,
        [
            sprite("bonus_loop", 3_000_000, 4_000_000, true),
            sprite("spin", 0, 500_000, false),
            sprite("stop", 1_500_000, 2_250_000, false),
        ]
    );
}

#[test]
fn a_howl_call_without_a_sprite_is_still_a_site() {
    let sites = report_of("pixi8-webgpu").howl_sites;
    let sources: Vec<&[String]> = sites.iter().map(|site| site.src.as_slice()).collect();
    assert_eq!(sources, [["assets/click.mp3"]]);
    assert!(sites[0].sprites.is_empty());
}

#[test]
fn a_howl_call_fed_from_json_is_not_a_site() {
    assert_eq!(report_of("pixi8-inline").howl_sites, []);
}

#[test]
fn the_engine_for_textures_is_the_pixi_major() {
    assert_eq!(report_of("pixi7-aa").engine, Engine::Pixi7);
    assert_eq!(report_of("pixi8-webgpu").engine, Engine::Pixi8);
    assert_eq!(report_of("howler-sprites").engine, Engine::Unknown);
}

#[test]
fn the_render_measures_come_from_the_render_site() {
    let measures = scan_measures("pixi7-aa");
    assert_eq!(
        value(&measures, MeasureId::StaticRenderAntialias),
        MeasureValue::Flag(true)
    );
    assert_eq!(
        value(&measures, MeasureId::StaticRenderResolutionUncapped),
        MeasureValue::Flag(true)
    );
    assert_eq!(
        value(&measures, MeasureId::StaticRenderBackend),
        MeasureValue::Text("webgl".into())
    );
    assert_eq!(
        value(&measures, MeasureId::StaticRenderCanvas),
        MeasureValue::Integer(12_648_528)
    );
}

#[test]
fn a_resolution_capped_at_2_is_not_uncapped() {
    let measures = scan_measures("pixi8-webgpu");
    assert_eq!(
        value(&measures, MeasureId::StaticRenderResolutionUncapped),
        MeasureValue::Flag(false)
    );
}

#[test]
fn the_backend_measure_is_what_the_site_asks_for_before_any_fallback() {
    let measures = scan_measures("pixi8-webgpu");
    assert_eq!(
        value(&measures, MeasureId::StaticRenderBackend),
        MeasureValue::Text("webgpu".into())
    );
}

#[test]
fn the_render_measures_are_missing_without_a_render_site_when_the_build_may_use_a_canvas() {
    // broken's script did not parse, so it may hold a canvas.
    let measures = scan_measures("broken");
    for id in [
        MeasureId::StaticRenderAntialias,
        MeasureId::StaticRenderResolutionUncapped,
        MeasureId::StaticRenderBackend,
        MeasureId::StaticRenderCanvas,
    ] {
        assert_eq!(
            measure(&measures, id).status,
            MeasureStatus::Missing,
            "{id:?}"
        );
    }
}

#[test]
fn the_engines_measure_counts_each_engine_found() {
    let measures = scan_measures("pixi8-inline");
    assert_eq!(
        value(&measures, MeasureId::StaticBundleEngines),
        MeasureValue::Integer(3)
    );
}

#[test]
fn each_library_is_keyed_by_its_bucket() {
    let measures = scan_measures("pixi8-inline");
    let engines = measure(&measures, MeasureId::StaticBundleEngines);
    let keys: Vec<&str> = engines.items.iter().map(|item| item.key.as_str()).collect();
    assert_eq!(keys, ["audio.howler", "rendering.pixi", "other.web_sdk"]);
}

#[test]
fn the_external_urls_measure_counts_urls_and_notes_hosts_built_at_run_time() {
    let measures = scan_measures("pixi8-inline");
    let urls = measure(&measures, MeasureId::StaticBundleExternalUrls);
    assert_eq!(urls.value, Some(MeasureValue::Integer(6)));
    assert_eq!(urls.items.len(), 6);
    assert_eq!(urls.scope.not_covered, ["hosts built at run time: 1"]);
}

#[test]
fn no_external_url_keeps_its_query_or_fragment() {
    let report = report_of("pixi8-inline");
    assert!(!report.external.urls.is_empty());
    for url in &report.external.urls {
        assert!(!url.url.contains(['?', '#']), "{}", url.url);
    }
}

#[test]
fn a_json_string_value_that_is_a_whole_url_counts() {
    let inventory = inventory_of(&[(
        "config.json",
        FileKind::Data,
        r#"{"api": {"base": "https://api.example.invalid/v1?key=abc"}, "note": "see https://x.invalid"}"#,
    )]);
    assert_eq!(url_list(&inventory), ["https://api.example.invalid/v1"]);
}

#[test]
fn a_json_file_with_a_byte_order_mark_is_read() {
    let inventory = inventory_of(&[(
        "config.json",
        FileKind::Data,
        "\u{feff}{\"cdn\": \"https://cdn.example.invalid/a\"}",
    )]);
    assert_eq!(url_list(&inventory), ["https://cdn.example.invalid/a"]);
}

#[test]
fn a_source_map_is_not_read_for_urls() {
    let source_map =
        r#"{"version": 3, "sourceRoot": "https://src.example.invalid/", "mappings": ""}"#;
    let inventory = inventory_of(&[
        ("main.js.map", FileKind::Data, source_map),
        ("LIB.JS.MAP", FileKind::Data, source_map),
    ]);
    assert_eq!(url_list(&inventory), Vec::<String>::new());
}

#[test]
fn an_import_map_in_html_counts_its_urls_and_is_not_a_script() {
    let inventory = inventory_of(&[(
        "index.html",
        FileKind::Html,
        concat!(
            r#"<script type=importmap>"#,
            r#"{"imports": {"pixi": "https://cdn.example.invalid/pixi.mjs"}}"#,
            "</script>",
        ),
    )]);
    assert_eq!(
        url_list(&inventory),
        ["https://cdn.example.invalid/pixi.mjs"]
    );
    assert_eq!(analyse(&inventory, &HostProfile::reels()).scanned, []);
}

#[test]
fn an_inline_script_offset_counts_bytes_after_a_byte_order_mark_and_crlf() {
    let html = concat!(
        "\u{feff}<!doctype html>\r\n<p>\u{e9}</p>\r\n",
        "<script>\r\napp.init({resolution: 2});\r\n</script>",
    );
    let inventory = inventory_of(&[("index.html", FileKind::Html, html)]);
    let report = analyse(&inventory, &HostProfile::reels());
    let offsets: Vec<u32> = report
        .render_configs
        .iter()
        .map(|config| config.offset)
        .collect();
    let object_start = html.find("{resolution").unwrap();
    assert_eq!(offsets, [u32::try_from(object_start).unwrap()]);
}

#[test]
fn a_script_that_sets_a_pixi_default_makes_an_absent_option_not_static() {
    let inventory = inventory_of(&[
        (
            "lib.js",
            FileKind::Js,
            "PIXI.settings.RESOLUTION = devicePixelRatio;",
        ),
        ("main.js", FileKind::Js, "app.init({antialias: !0});"),
    ]);
    let scan = bundle::scan(&inventory, &HostProfile::reels());
    let uncapped = measure(&scan.measures, MeasureId::StaticRenderResolutionUncapped);
    assert_eq!(uncapped.status, MeasureStatus::Unsupported);
    assert_eq!(uncapped.reason.as_deref(), Some("option_not_static"));
    let antialias = measure(&scan.measures, MeasureId::StaticRenderAntialias);
    assert_eq!(antialias.value, Some(MeasureValue::Flag(true)));
}

#[test]
fn a_build_with_no_canvas_engine_or_external_url_draws_with_html() {
    let inventory = inventory_of(&[
        (
            "index.html",
            FileKind::Html,
            "<script src='main.js'></script>",
        ),
        ("main.js", FileKind::Js, "document.body.append('hello')"),
    ]);
    let scan = bundle::scan(&inventory, &HostProfile::reels());
    assert!(scan.summary.draws_with_html);
    let without_a_page =
        inventory_of(&[("main.js", FileKind::Js, "document.body.append('hello')")]);
    let scan_without_a_page = bundle::scan(&without_a_page, &HostProfile::reels());
    assert!(!scan_without_a_page.summary.draws_with_html);
    let backend = measure(&scan.measures, MeasureId::StaticRenderBackend);
    assert_eq!(backend.value, Some(MeasureValue::Text("none".into())));
    let canvas = measure(&scan.measures, MeasureId::StaticRenderCanvas);
    assert_eq!(canvas.status, MeasureStatus::Unsupported);
}

#[test]
fn howler_alone_does_not_stop_a_build_drawing_with_html() {
    // The label is about the rendering surface, and Howler only plays audio.
    let page = (
        "index.html",
        FileKind::Html,
        "<script src='main.js'></script>",
    );
    let scripts = [
        "/*! howler.js v2.2.4 */ new Howl({src: ['a.mp3']})",
        "const options = {html5PoolSize: 10}; document.body.append('hello')",
    ];
    for script in scripts {
        let inventory = inventory_of(&[page, ("main.js", FileKind::Js, script)]);
        let scan = bundle::scan(&inventory, &HostProfile::reels());
        assert!(scan.summary.draws_with_html, "{script}");
    }
    let howler_and_pixi = inventory_of(&[
        page,
        (
            "main.js",
            FileKind::Js,
            "/*! howler.js v2.2.4 */ const v = `PixiJS 8.22.0 - ${r} - http://www.pixijs.com/`",
        ),
    ]);
    let scan = bundle::scan(&howler_and_pixi, &HostProfile::reels());
    assert!(
        !scan.summary.draws_with_html,
        "a rendering engine still disqualifies"
    );
}

#[test]
fn only_a_rendering_library_stops_a_build_drawing_with_html() {
    // web-sdk is in the other bucket: alone, with no canvas, the page draws with HTML. A real
    // web-sdk build bundles Pixi, which is in the rendering bucket.
    let web_sdk = "const env = __sveltekit_x.env; \
                   fetch(params.get('rgs_url') + '/wallet/authenticate'); \
                   fetch(base + '/wallet/play');";
    let page = (
        "index.html",
        FileKind::Html,
        "<script src='main.js'></script>",
    );
    let inventory = inventory_of(&[page, ("main.js", FileKind::Js, web_sdk)]);
    let scan = bundle::scan(&inventory, &HostProfile::reels());
    let engines = measure(&scan.measures, MeasureId::StaticBundleEngines);
    let keys: Vec<&str> = engines.items.iter().map(|item| item.key.as_str()).collect();
    assert_eq!(keys, ["other.web_sdk"]);
    assert!(scan.summary.draws_with_html);
}

#[test]
fn the_howler_fixture_draws_with_html() {
    let measures = scan_measures("howler-sprites");
    let backend = measure(&measures, MeasureId::StaticRenderBackend);
    assert_eq!(backend.value, Some(MeasureValue::Text("none".into())));
}

#[test]
fn canvas_use_a_rendering_engine_a_broken_script_or_an_external_url_keeps_the_render_measures_missing()
 {
    let builds = [
        "document.querySelector('x').getContext('2d')",
        "const c = document.querySelector('x'); c.getContext.call(c, '2d')",
        "document.body.append(document.createElement('canvas'))",
        "const v = `PixiJS 8.22.0 - ${r} - http://www.pixijs.com/`",
        "fetch('https://cdn.example.invalid/x.js')",
        "const = ;",
    ];
    for script in builds {
        let inventory = inventory_of(&[
            (
                "index.html",
                FileKind::Html,
                "<script src='main.js'></script>",
            ),
            ("main.js", FileKind::Js, script),
        ]);
        let scan = bundle::scan(&inventory, &HostProfile::reels());
        assert!(!scan.summary.draws_with_html, "{script}");
        let canvas = measure(&scan.measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.status, MeasureStatus::Missing, "{script}");
    }
    let canvas_in_markup = inventory_of(&[("index.html", FileKind::Html, "<canvas></canvas>")]);
    let scan = bundle::scan(&canvas_in_markup, &HostProfile::reels());
    assert!(!scan.summary.draws_with_html);
}

#[test]
fn css_inside_html_counts_its_urls() {
    let inventory = inventory_of(&[(
        "index.html",
        FileKind::Html,
        "<style>a{b:url(https://cdn.example.invalid/a.png)}</style><p style='c:url(//cdn.example.invalid/b.png)'>",
    )]);
    assert_eq!(
        url_list(&inventory),
        [
            "//cdn.example.invalid/b.png",
            "https://cdn.example.invalid/a.png"
        ]
    );
}

#[test]
fn an_inline_script_that_does_not_parse_makes_its_html_file_invalid() {
    let inventory = inventory_of(&[(
        "index.html",
        FileKind::Html,
        "<script>ok()</script><script>const = ;</script>",
    )]);
    let report = analyse(&inventory, &HostProfile::reels());
    assert_eq!(report.scanned, [path("index.html")]);
    assert_eq!(report.invalid, [path("index.html")]);
}

#[test]
fn a_script_that_does_not_parse_is_named_in_every_built_measure_scope() {
    let broken = fixture("broken");
    let scan = bundle::scan(&broken.inventory(), &broken.profile());
    assert_eq!(scan.summary.invalid_scripts, [path("bad/bad-syntax.js")]);
    let built = bundle::MEASURES
        .iter()
        .filter(|id| **id != MeasureId::StaticBundleSourceMap);
    for &id in built {
        assert_eq!(
            measure(&scan.measures, id).scope.not_covered,
            ["a script that did not parse: bad/bad-syntax.js"],
            "{id:?}"
        );
    }
}

#[test]
fn the_source_map_measure_is_not_built_yet() {
    let measures = scan_measures("pixi7-aa");
    let source_map = measure(&measures, MeasureId::StaticBundleSourceMap);
    assert_eq!(source_map.status, MeasureStatus::Missing);
    assert_eq!(source_map.reason.as_deref(), Some(NOT_BUILT_YET));
}

#[test]
fn the_summary_holds_one_render_site_per_config() {
    let opts = fixture("pixi7-opts");
    let scan = bundle::scan(&opts.inventory(), &opts.profile());
    let sites = &scan.summary.render_sites[..];
    let [site] = sites else {
        panic!("one render site, not {}", sites.len());
    };
    assert_eq!(site.resolution.fixed(), Some(3_000));
    assert_eq!(site.canvas.map(|canvas| canvas.bytes), Some(33_177_600));
}

#[test]
fn the_stub_inventory_keeps_every_measure_missing_as_not_built_yet() {
    let scan = bundle::scan(&Inventory::default(), &HostProfile::reels());
    assert_eq!(scan.measures.len(), bundle::MEASURES.len());
    for measure in &scan.measures {
        assert_eq!(
            measure.reason.as_deref(),
            Some(NOT_BUILT_YET),
            "{:?}",
            measure.id
        );
    }
}

/// Each folder fixture with its report and `[bundle]` table. A zip needs the inventory.
fn folder_fixtures() -> Vec<(Fixture, BundleReport, Table)> {
    let folders: Vec<(Fixture, BundleReport, Table)> = fixtures()
        .into_iter()
        .filter(|fixture| !fixture.is_zip())
        .map(|fixture| {
            let expected = fixture.required_table("bundle").clone();
            let report = analyse(&fixture.inventory(), &fixture.profile());
            (fixture, report, expected)
        })
        .collect();
    assert!(!folders.is_empty(), "no folder fixtures");
    folders
}

fn paths(files: &[RelPath]) -> Value {
    Value::from(files.iter().map(RelPath::as_str).collect::<Vec<_>>())
}

/// `[bundle.versions]`: a key per engine found.
fn versions_table(versions: &EngineVersions) -> Value {
    let mut table = Table::new();
    for (key, version) in [("pixi", &versions.pixi), ("howler", &versions.howler)] {
        if let Some(version) = version {
            table.insert(key.into(), Value::from(version.as_text()));
        }
    }
    table.insert("web_sdk".into(), Value::from(versions.web_sdk));
    if versions.web_sdk {
        let unsupported = Version::Unsupported;
        table.insert("web_sdk_version".into(), Value::from(unsupported.as_text()));
    }
    Value::Table(table)
}

/// `[[bundle.render_site]]` rows, without the resolution note.
fn render_rows(configs: &[RenderConfig]) -> Value {
    let rows = configs.iter().map(|config| {
        let mut row = Table::new();
        let canvas = config.canvas.expect("every fixture site has a canvas");
        let pair = |a: u32, b: u32| Value::from(vec![a, b]);
        row.insert("file".into(), Value::from(config.file.as_str()));
        row.insert("callee".into(), Value::from(config.callee.as_text()));
        row.insert(
            "antialias".into(),
            Value::from(config.antialias.expect("a static antialias")),
        );
        row.insert(
            "antialias_from".into(),
            Value::from(config.antialias_from.as_text()),
        );
        let resolution = config.resolution.fixed().expect("a static resolution");
        row.insert("resolution_milli".into(), Value::from(resolution));
        let preference = config.preference.expect("a static preference");
        row.insert("preference".into(), Value::from(preference.as_text()));
        row.insert(
            "modelled_as".into(),
            Value::from(
                config
                    .modelled_as
                    .expect("every fixture site has a backend")
                    .as_text(),
            ),
        );
        row.insert(
            "auto_density".into(),
            Value::from(config.auto_density.expect("a static autoDensity")),
        );
        row.insert("size_from".into(), Value::from(config.size_from.as_text()));
        let css = config.canvas_css;
        row.insert("canvas_css".into(), pair(css.width, css.height));
        row.insert(
            "canvas_backing".into(),
            pair(canvas.backing_width, canvas.backing_height),
        );
        row.insert("canvas_pixels".into(), integer_value(canvas.pixels));
        row.insert("canvas_bytes".into(), integer_value(canvas.bytes));
        Value::Table(row)
    });
    Value::Array(rows.collect())
}

fn integer_value(number: u64) -> Value {
    Value::Integer(i64::try_from(number).expect("fits in a TOML integer"))
}

/// `[bundle.external]`, with its `[[bundle.external.url]]` rows when there are any.
fn external_table(external: &ExternalUrls) -> Value {
    let mut table = Table::new();
    table.insert("hosts".into(), Value::from(external.hosts.clone()));
    table.insert(
        "dynamic_host_sites".into(),
        Value::from(external.dynamic_host_sites),
    );
    if !external.urls.is_empty() {
        let rows = external.urls.iter().map(|url| {
            let mut row = Table::new();
            row.insert("url".into(), Value::from(url.url.as_str()));
            row.insert("sources".into(), paths(&url.sources));
            Value::Table(row)
        });
        table.insert("url".into(), Value::Array(rows.collect()));
    }
    Value::Table(table)
}

fn report_of(name: &str) -> BundleReport {
    let fixture = fixture(name);
    analyse(&fixture.inventory(), &fixture.profile())
}

fn path(raw: &str) -> RelPath {
    RelPath::new(raw).unwrap()
}

fn scan_measures(name: &str) -> Vec<Measure> {
    let fixture = fixture(name);
    bundle::scan(&fixture.inventory(), &fixture.profile()).measures
}

fn value(measures: &[Measure], id: MeasureId) -> MeasureValue {
    measure(measures, id)
        .value
        .clone()
        .unwrap_or_else(|| panic!("{id:?} has no value"))
}

fn measure(measures: &[Measure], id: MeasureId) -> &Measure {
    measures
        .iter()
        .find(|measure| measure.id == id)
        .unwrap_or_else(|| panic!("no {id:?}"))
}

/// An inventory of these files, for cases no fixture has.
fn inventory_of(files: &[(&str, FileKind, &str)]) -> Inventory {
    let manifest_files = files
        .iter()
        .map(|(raw, kind, text)| ManifestFile {
            path: path(raw),
            bytes: text.len() as u64,
            sha256: String::new(),
            kind: *kind,
        })
        .collect();
    let contents = files
        .iter()
        .map(|(raw, _, text)| (path(raw), text.as_bytes().to_vec()))
        .collect();
    let manifest = BuildManifest {
        build_id: None,
        files: manifest_files,
    };
    Inventory::new(manifest, contents)
}

/// The external URLs the bundle scan finds in `inventory`.
fn url_list(inventory: &Inventory) -> Vec<String> {
    let report = analyse(inventory, &HostProfile::reels());
    report
        .external
        .urls
        .into_iter()
        .map(|url| url.url)
        .collect()
}
