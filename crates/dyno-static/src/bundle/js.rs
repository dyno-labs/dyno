//! JavaScript: parses one script with oxc and reads its render configs, Howl sites, external URLs,
//! engine markers and writes of Pixi defaults (design section 8.1).

use std::collections::HashSet;
use std::{panic, thread};

use dyno_schema::path::RelPath;
use dyno_schema::profile::HostProfile;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentExpression, CallExpression, ComputedMemberExpression, ExportAllDeclaration,
    ExportFromDeclaration, Expression, IdentifierReference, ImportDeclaration, ImportExpression,
    NewExpression, ObjectProperty, Program, StaticMemberExpression, StringLiteral, TemplateLiteral,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

use crate::bundle::eval::{Evaluator, ObjectKeys};
use crate::bundle::howl;
use crate::bundle::render::{self, Callee, DefaultWrites, RenderConfig, SiteAt};
use crate::bundle::requests::{self, RequestText};
use crate::bundle::urls::{self, ExternalUrl, TemplateStart};
use crate::bundle::versions::EngineMarkers;
use crate::model::HowlSite;

/// What one script holds.
#[derive(Debug, Default)]
pub struct ScriptScan {
    /// In source order.
    pub render_configs: Vec<RenderConfig>,
    /// In source order.
    pub howl_sites: Vec<HowlSite>,
    /// The URLs in request positions, in source order, with repeats.
    pub urls: Vec<ExternalUrl>,
    /// Request positions whose host is built at run time.
    pub dynamic_host_sites: u32,
    pub markers: EngineMarkers,
    pub default_writes: DefaultWrites,
    /// Whether the script may use a canvas: it reads a canvas context method on any object (a
    /// call, `.call`, `.bind` or an alias), names a canvas class, or holds the string `canvas` or
    /// a `<canvas` tag.
    pub uses_a_canvas: bool,
}

/// The script cannot be scanned: it parses neither as a module nor as a classic script, it nests
/// deeper than the scan allows, or no thread with a large stack is free.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidScript;

/// Where a script's text is.
#[derive(Clone, Copy)]
pub struct ScriptAt<'f> {
    /// The script, or the HTML file of an inline script.
    pub file: &'f RelPath,
    /// The byte offset of the script's text from the start of `file`.
    pub offset: u32,
}

/// Walks the syntax tree once and collects everything.
struct Reader<'f, 'p, 's, 'a> {
    at: ScriptAt<'f>,
    /// The script's text.
    text: &'a str,
    profile: &'p HostProfile,
    evaluator: Evaluator<'s, 'a>,
    scan: ScriptScan,
    /// Each object read so far, with the kind of call it was passed to.
    objects_read: HashSet<(u32, Callee)>,
    /// The offset of each render config's object.
    config_offsets: HashSet<u32>,
    /// Where each template or concatenation that builds a host at run time starts.
    dynamic_host_starts: HashSet<u32>,
}

/// Property names that reach a canvas or its drawing context. Only a property counts, not a
/// plain name: Svelte has its own `getContext` function, called with no object.
const CANVAS_PROPERTIES: &[&str] = &[
    "getContext",
    "transferControlToOffscreen",
    "OffscreenCanvas",
    "HTMLCanvasElement",
];

/// Global classes of canvases.
const CANVAS_CLASSES: &[&str] = &["OffscreenCanvas", "HTMLCanvasElement"];

/// The element name, as `createElement` takes it.
const CANVAS_ELEMENT: &str = "canvas";

/// A canvas tag in markup a script writes.
const CANVAS_TAG: &str = "<canvas";

// oxc parses and walks nested code by recursion, with no depth limit, and a stack overflow
// aborts the process. So the scan runs on a thread with a large stack, and a script nested
// deeper than the limit counts as invalid.
const SCAN_STACK_BYTES: usize = 64 * 1024 * 1024;
const MAX_BRACKET_DEPTH: usize = 1_000;

/// Reads one script. `text` is the script alone, so an inline script is parsed without its
/// HTML.
pub fn scan_script(
    at: ScriptAt<'_>,
    text: &str,
    profile: &HostProfile,
) -> Result<ScriptScan, InvalidScript> {
    if bracket_depth(text) > MAX_BRACKET_DEPTH {
        return Err(InvalidScript);
    }
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .stack_size(SCAN_STACK_BYTES)
            .spawn_scoped(scope, move || read_script(at, text, profile));
        match worker {
            Ok(worker) => worker
                .join()
                .unwrap_or_else(|payload| panic::resume_unwind(payload)),
            // This thread's smaller stack could overflow, which cannot be caught.
            Err(_) => Err(InvalidScript),
        }
    })
}

/// Parses `text` as a module, then as a classic script. None when both fail.
pub(crate) fn parse<'a>(allocator: &'a Allocator, text: &'a str) -> Option<&'a Program<'a>> {
    // Drop parenthesis nodes, so the site rules see an object literal itself.
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    for source_type in [SourceType::mjs(), SourceType::script()] {
        let parsed = Parser::new(allocator, text, source_type)
            .with_options(options)
            .parse();
        if !parsed.diagnostics.has_errors() {
            return Some(allocator.alloc(parsed.program));
        }
    }
    None
}

/// The deepest nesting of `(`, `[` and `{` in the raw text, strings and comments included.
fn bracket_depth(text: &str) -> usize {
    let mut depth: usize = 0;
    let mut deepest = 0;
    for byte in text.bytes() {
        match byte {
            b'(' | b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

fn read_script(
    at: ScriptAt<'_>,
    text: &str,
    profile: &HostProfile,
) -> Result<ScriptScan, InvalidScript> {
    let allocator = Allocator::default();
    let program = parse(&allocator, text).ok_or(InvalidScript)?;
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let mut reader = Reader {
        at,
        text,
        profile,
        evaluator: Evaluator::new(&semantic, f64::from(profile.device_scale_factor)),
        scan: ScriptScan::default(),
        objects_read: HashSet::new(),
        config_offsets: HashSet::new(),
        dynamic_host_starts: HashSet::new(),
    };
    reader.visit_program(program);
    for comment in &program.comments {
        let span = comment.span;
        reader
            .scan
            .markers
            .read_comment(&text[span.start as usize..span.end as usize]);
    }
    Ok(reader.scan)
}

impl<'a> Reader<'_, '_, '_, 'a> {
    /// Notes a canvas from text: the element name, as `createElement("canvas")` takes it, or a
    /// canvas tag in markup.
    fn note_canvas_text(&mut self, text: &str) {
        if text.eq_ignore_ascii_case(CANVAS_ELEMENT)
            || text.to_ascii_lowercase().contains(CANVAS_TAG)
        {
            self.scan.uses_a_canvas = true;
        }
    }

    /// Reads the render config in the first argument of a call or `new`, and the Howl site in
    /// the first argument of a `new`.
    fn read_call(&mut self, callee: Callee, call_start: u32, arguments: &[Argument<'a>]) {
        let Some(first) = arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        let Some(object) = self.evaluator.object(first) else {
            return;
        };
        let object_offset = self.at.offset.saturating_add(object.span.start);
        // The rules depend only on the keys and the kind of call, so a repeat reads nothing new.
        // Skipping it keeps the work linear when one object is passed to many calls.
        if !self.objects_read.insert((object_offset, callee)) {
            return;
        }
        let keys = self.evaluator.keys(object);
        // One object passed to two calls is one site.
        if !self.config_offsets.contains(&object_offset) {
            self.read_render_config(object_offset, callee, &keys);
        }
        if callee == Callee::New {
            let call_offset = self.at.offset.saturating_add(call_start);
            let howl_site = howl::read_site(self.at.file, call_offset, &keys, &self.evaluator);
            self.scan.howl_sites.extend(howl_site);
        }
    }

    fn read_render_config(&mut self, offset: u32, callee: Callee, keys: &ObjectKeys<'_, 'a>) {
        let site = SiteAt {
            file: self.at.file,
            offset,
            callee,
        };
        let config = render::read_config(site, keys, &self.evaluator, self.profile, self.text);
        if let Some(config) = config {
            self.config_offsets.insert(offset);
            self.scan.render_configs.push(config);
        }
    }

    /// The URLs that a request position can be given.
    fn read_request(&mut self, expression: &Expression<'a>) {
        for text in requests::request_texts(&self.evaluator, expression) {
            self.read_request_text(&text);
        }
    }

    fn read_request_text(&mut self, text: &RequestText) {
        match text {
            RequestText::Whole(text) => self.scan.urls.extend(urls::whole_url(text)),
            RequestText::Start { text, site } => match urls::template_start(text) {
                TemplateStart::Url(url) => self.scan.urls.push(url),
                TemplateStart::DynamicHost => {
                    if self.dynamic_host_starts.insert(*site) {
                        self.scan.dynamic_host_sites =
                            self.scan.dynamic_host_sites.saturating_add(1);
                    }
                }
                TemplateStart::NotAUrl => {}
            },
        }
    }

    /// The source of a static `import` or `export ... from`.
    fn read_module_source(&mut self, source: &StringLiteral<'a>) {
        if let Some(text) = source.value.as_str() {
            self.read_request_text(&RequestText::Whole(text.to_owned()));
        }
    }
}

impl<'a> Visit<'a> for Reader<'_, '_, '_, 'a> {
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.read_call(Callee::New, it.span.start, &it.arguments);
        for argument in requests::requested_arguments(&it.callee, &it.arguments, &self.evaluator) {
            self.read_request(argument);
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.read_call(Callee::of_call(&it.callee), it.span.start, &it.arguments);
        for argument in requests::requested_arguments(&it.callee, &it.arguments, &self.evaluator) {
            self.read_request(argument);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        self.read_request(&it.source);
        walk::walk_import_expression(self, it);
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        self.read_module_source(&it.source);
        walk::walk_import_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) {
        self.read_module_source(&it.source);
        walk::walk_export_all_declaration(self, it);
    }

    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) {
        self.read_module_source(&it.source);
        walk::walk_export_from_declaration(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let Some(option) = render::default_set_by(&it.left) {
            let code = render::source_text(it, self.text);
            self.scan.default_writes.add(option, code);
        }
        let property = it
            .left
            .as_member_expression()
            .and_then(|member| member.static_property_name());
        if property
            .and_then(|name| name.as_str())
            .is_some_and(requests::is_resource_key)
        {
            self.read_request(&it.right);
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if let Some(text) = it.value.as_str() {
            self.scan.markers.read_literal(text);
            self.note_canvas_text(text);
        }
    }

    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        for quasi in &it.quasis {
            if let Some(text) = quasi.value.cooked.as_ref().and_then(|text| text.as_str()) {
                self.note_canvas_text(text);
            }
        }
        if !it.expressions.is_empty() {
            self.scan.markers.read_template(it, &self.evaluator);
        } else if let Some(text) = it.single_quasi().and_then(|quasi| quasi.as_str()) {
            self.scan.markers.read_literal(text);
        }
        walk::walk_template_literal(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.scan.markers.read_property_name(&it.property.name);
        if CANVAS_PROPERTIES.contains(&it.property.name.as_str()) {
            self.scan.uses_a_canvas = true;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::StringLiteral(key) = it.expression.without_parentheses()
            && key
                .value
                .as_str()
                .is_some_and(|name| CANVAS_PROPERTIES.contains(&name))
        {
            self.scan.uses_a_canvas = true;
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if let Some(name) = it.key.static_name()
            && let Some(name) = name.as_str()
        {
            self.scan.markers.read_object_key(name);
            if requests::is_resource_key(name) {
                self.read_request(&it.value);
            }
        }
        walk::walk_object_property(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if CANVAS_CLASSES.contains(&it.name.as_str()) {
            self.scan.uses_a_canvas = true;
        }
        if self.evaluator.is_global(it) {
            self.scan.markers.read_global_name(&it.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_at(source: &str, offset: u32) -> Result<ScriptScan, InvalidScript> {
        let file = RelPath::new("index.html").unwrap();
        let at = ScriptAt {
            file: &file,
            offset,
        };
        scan_script(at, source, &HostProfile::reels())
    }

    fn scan(source: &str) -> ScriptScan {
        scan_at(source, 0).unwrap()
    }

    fn urls(source: &str) -> Vec<String> {
        scan(source).urls.into_iter().map(|url| url.url).collect()
    }

    #[test]
    fn a_script_valid_only_as_a_classic_script_parses() {
        // `await` is reserved in a module, not in a classic script.
        assert!(scan_at("var await = 1;", 0).is_ok());
    }

    #[test]
    fn a_script_that_parses_neither_way_is_invalid() {
        assert_eq!(scan_at("const = ;", 0).unwrap_err(), InvalidScript);
    }

    #[test]
    fn a_script_nested_to_the_limit_parses() {
        let depth = MAX_BRACKET_DEPTH;
        let script = format!("{}{}", "f(".repeat(depth), ")".repeat(depth));
        assert!(scan_at(&script, 0).is_ok());
    }

    #[test]
    fn a_script_nested_deeper_than_the_limit_is_invalid() {
        let depth = MAX_BRACKET_DEPTH + 1;
        let script = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(scan_at(&script, 0).unwrap_err(), InvalidScript);
    }

    #[test]
    fn offsets_count_from_the_start_of_the_file() {
        let scan = scan_at("app.init({resolution: 1}); new Howl({src: 'a.mp3'})", 100).unwrap();
        assert_eq!(scan.render_configs[0].offset, 109);
        assert_eq!(scan.howl_sites[0].offset, 127);
    }

    #[test]
    fn a_whole_url_outside_a_request_position_does_not_count() {
        let source = concat!(
            "console.warn('https://docs.example.invalid/e/a');",
            "throw Error(`https://docs.example.invalid/e/b`);",
            "const page = 'https://docs.example.invalid/e/c';",
        );
        assert_eq!(urls(source), Vec::<String>::new());
    }

    #[test]
    fn a_url_inside_a_longer_literal_does_not_count() {
        assert!(urls("fetch('see https://c.example.invalid')").is_empty());
    }

    #[test]
    fn every_argument_of_a_request_call_counts() {
        let calls = [
            "fetch('https://a.example.invalid/1')",
            "x.open('GET', 'https://a.example.invalid/2')",
            "navigator.sendBeacon('https://a.example.invalid/3')",
            "importScripts('https://a.example.invalid/4')",
            "navigator.serviceWorker.register('https://a.example.invalid/5')",
            "Assets.load('https://a.example.invalid/6')",
            "Texture.from('https://a.example.invalid/7')",
            "new URL('/8', 'https://a.example.invalid/')",
            "new Request('https://a.example.invalid/9')",
            "new WebSocket('wss://a.example.invalid/10')",
            "new EventSource('https://a.example.invalid/11')",
            "new Worker('https://a.example.invalid/12')",
            "new SharedWorker('https://a.example.invalid/13')",
            "new Audio('https://a.example.invalid/14')",
        ];
        for call in calls {
            assert_eq!(urls(call).len(), 1, "{call}");
        }
    }

    #[test]
    fn a_method_called_through_a_comma_list_is_a_request() {
        assert_eq!(
            urls("(0, w.fetch)('https://a.example.invalid/x')"),
            ["https://a.example.invalid/x"]
        );
    }

    #[test]
    fn an_import_counts_its_source() {
        let source = concat!(
            "import 'https://a.example.invalid/a.mjs';",
            "export * from 'https://b.example.invalid/b.mjs';",
            "export { c } from 'https://c.example.invalid/c.mjs';",
            "import('https://d.example.invalid/d.mjs');",
        );
        assert_eq!(
            urls(source),
            [
                "https://a.example.invalid/a.mjs",
                "https://b.example.invalid/b.mjs",
                "https://c.example.invalid/c.mjs",
                "https://d.example.invalid/d.mjs",
            ]
        );
    }

    #[test]
    fn the_value_of_a_resource_key_counts() {
        let source = concat!(
            "new Howl({src: ['https://a.example.invalid/a.mp3']});",
            "f({href: 'https://b.example.invalid/b', url: 'https://c.example.invalid/c'});",
            "f({poster: 'https://d.example.invalid/d.png', srcset: 'https://e.example.invalid/e.png'});",
            "f({docs: 'https://f.example.invalid/f'});",
        );
        assert_eq!(
            urls(source),
            [
                "https://a.example.invalid/a.mp3",
                "https://b.example.invalid/b",
                "https://c.example.invalid/c",
                "https://d.example.invalid/d.png",
                "https://e.example.invalid/e.png",
            ]
        );
    }

    #[test]
    fn a_value_assigned_to_a_resource_property_counts() {
        let source = concat!(
            "img.src = 'https://a.example.invalid/a.png';",
            "link.href = 'https://b.example.invalid/b';",
            "note.text = 'https://c.example.invalid/c';",
        );
        assert_eq!(
            urls(source),
            [
                "https://a.example.invalid/a.png",
                "https://b.example.invalid/b"
            ]
        );
    }

    #[test]
    fn set_attribute_requests_only_a_resource_attribute() {
        let source = concat!(
            "e.setAttribute('SRC', 'https://a.example.invalid/a.png');",
            "e.setAttribute('data-docs', 'https://b.example.invalid/b');",
        );
        assert_eq!(urls(source), ["https://a.example.invalid/a.png"]);
    }

    #[test]
    fn a_request_is_followed_through_wrappers() {
        let source = concat!(
            "fetch(c ? 'https://a.example.invalid/a' : 'https://b.example.invalid/b');",
            "fetch(d || 'https://c.example.invalid/c');",
            "fetch('https://d.example.invalid/d' ?? d);",
            "fetch((g(), 'https://e.example.invalid/e'));",
            "Assets.load(['https://f.example.invalid/f']);",
        );
        assert_eq!(
            urls(source),
            [
                "https://a.example.invalid/a",
                "https://b.example.invalid/b",
                "https://c.example.invalid/c",
                "https://d.example.invalid/d",
                "https://e.example.invalid/e",
                "https://f.example.invalid/f",
            ]
        );
    }

    #[test]
    fn a_request_is_followed_to_a_variable_written_once() {
        let source = "const u = 'https://a.example.invalid/x'; console.log(u); fetch(u);";
        assert_eq!(urls(source), ["https://a.example.invalid/x"]);
    }

    #[test]
    fn a_request_is_followed_to_a_key_of_an_object_bound_to_a_variable() {
        let source = "const c = {api: 'https://a.example.invalid/v1/'}; fetch(c.api + p);";
        assert_eq!(urls(source), ["https://a.example.invalid/v1/"]);
    }

    #[test]
    fn a_concatenated_url_keeps_its_left_literal() {
        assert_eq!(
            urls("fetch('https://api.example.invalid/v1/config/' + s + '.json')"),
            ["https://api.example.invalid/v1/config/"]
        );
    }

    #[test]
    fn a_concatenation_that_builds_the_host_counts_a_dynamic_host_site() {
        let scan = scan("fetch('https://api.' + env + '/v1')");
        assert_eq!(scan.urls, []);
        assert_eq!(scan.dynamic_host_sites, 1);
    }

    #[test]
    fn a_concatenation_of_known_text_is_one_whole_url() {
        let scan = scan("const h = 'example.invalid'; fetch('https://api.' + h + '/x')");
        assert_eq!(scan.dynamic_host_sites, 0);
        let urls: Vec<String> = scan.urls.into_iter().map(|url| url.url).collect();
        assert_eq!(urls, ["https://api.example.invalid/x"]);
    }

    #[test]
    fn a_host_built_outside_a_request_position_is_not_a_dynamic_host_site() {
        let scan = scan("throw Error(`https://${host}/e`); console.warn('https://' + h);");
        assert_eq!(scan.dynamic_host_sites, 0);
    }

    #[test]
    fn the_left_literal_of_a_concatenation_feeds_the_engine_markers() {
        let scan = scan("f('/wallet/play' + q)");
        assert!(scan.markers.rgs_paths.contains("/wallet/play"));
    }

    #[test]
    fn a_template_with_a_whole_host_records_its_start() {
        assert_eq!(
            urls("fetch(`https://cdn.example.invalid/a/${n}.png`)"),
            ["https://cdn.example.invalid/a/"]
        );
    }

    #[test]
    fn a_template_with_no_expressions_is_a_whole_url() {
        let scan = scan("fetch(`https://a.example.invalid`)");
        assert_eq!(scan.dynamic_host_sites, 0);
        assert_eq!(scan.urls.len(), 1);
    }

    #[test]
    fn a_template_with_no_expressions_feeds_the_engine_markers() {
        let scan = scan("f(`/wallet/play`)");
        assert!(scan.markers.rgs_paths.contains("/wallet/play"));
    }

    #[test]
    fn a_template_that_builds_the_host_counts_a_dynamic_host_site() {
        let scan = scan("fetch(`https://${host}${path}`)");
        assert_eq!(scan.urls, []);
        assert_eq!(scan.dynamic_host_sites, 1);
    }

    #[test]
    fn a_part_read_as_one_whole_text_is_joined_in() {
        let source = concat!(
            "const cdn = 'https://cdn.example.invalid';",
            "const h = 'api.example.invalid';",
            "fetch(`${cdn}/a.png`); fetch(`https://${h}/x/${n}`);",
            "fetch('https://' + ('cdn.example.invalid' + '/b.png'));",
        );
        let scan = scan(source);
        assert_eq!(scan.dynamic_host_sites, 0);
        let urls: Vec<String> = scan.urls.into_iter().map(|url| url.url).collect();
        assert_eq!(
            urls,
            [
                "https://cdn.example.invalid/a.png",
                "https://api.example.invalid/x/",
                "https://cdn.example.invalid/b.png"
            ]
        );
    }

    #[test]
    fn a_template_that_starts_with_an_expression_starts_with_what_it_reads_as() {
        let source = concat!(
            "const c = {api: 'https://api.example.invalid/v1'}; fetch(`${c.api}/x`);",
            "const b = `https://${h}`; fetch(`${b}/y`);",
        );
        let scan = scan(source);
        assert_eq!(scan.dynamic_host_sites, 1);
        let urls: Vec<String> = scan.urls.into_iter().map(|url| url.url).collect();
        assert_eq!(urls, ["https://api.example.invalid/v1/x"]);
    }

    #[test]
    fn a_host_built_at_run_time_is_kept_before_a_part_too_long_to_read() {
        let long = "x".repeat(9_000);
        let source = format!("const t = '{long}'; const b = `https://${{h}}`; fetch(b + t);");
        assert_eq!(scan(&source).dynamic_host_sites, 1);
    }

    #[test]
    fn a_host_built_at_run_time_counts_once_however_it_is_reached() {
        let source = concat!(
            "const c = {url: `https://${h}/x`}; fetch(c.url);",
            "const b = 'https://' + g; fetch(x ? b + 'a' : b + 'b'); img.src = b;",
        );
        assert_eq!(scan(source).dynamic_host_sites, 2);
    }

    #[test]
    fn two_templates_or_concatenations_that_build_a_host_are_two_sites() {
        assert_eq!(
            scan("fetch('https://' + a); fetch('https://' + a);").dynamic_host_sites,
            2
        );
        assert_eq!(
            scan("fetch(`https://${a}`); fetch(`https://${a}`);").dynamic_host_sites,
            2
        );
    }

    #[test]
    fn a_long_concatenation_keeps_its_start_and_its_known_parts() {
        let parameters: String = (0..40).map(|i| format!(" + '&p{i}=' + p{i}")).collect();
        let source = format!("fetch('https://' + 'api.example.invalid' + '/spin?a=1'{parameters})");
        assert_eq!(urls(&source), ["https://api.example.invalid/spin"]);
    }

    #[test]
    fn a_write_of_pixi_7_settings_resolution_is_a_default_write() {
        let scan = scan("PIXI.settings.RESOLUTION = devicePixelRatio;");
        assert_eq!(
            scan.default_writes.resolution.as_deref(),
            Some("PIXI.settings.RESOLUTION = devicePixelRatio")
        );
    }

    #[test]
    fn a_write_of_default_options_antialias_is_a_default_write() {
        let scan = scan("ViewSystem.defaultOptions.antialias = !0;");
        assert!(scan.default_writes.antialias.is_some());
    }

    #[test]
    fn a_write_of_pixi_7_0_render_options_antialias_is_a_default_write() {
        let scan = scan("PIXI.settings.RENDER_OPTIONS.antialias = !0;");
        assert!(scan.default_writes.antialias.is_some());
    }

    #[test]
    fn a_write_of_another_default_option_is_not_a_default_write() {
        let scan = scan("V.defaultOptions.bezierSmoothness = .5;");
        assert_eq!(scan.default_writes, DefaultWrites::default());
    }

    #[test]
    fn a_write_to_an_instance_option_is_not_a_default_write() {
        let scan = scan("app.renderer.antialias = !0; app.resolution = 2;");
        assert_eq!(scan.default_writes, DefaultWrites::default());
    }

    #[test]
    fn an_object_passed_to_new_twice_is_one_howl_site() {
        let scan = scan("const o = {src: ['a.mp3']}; new Howl(o); new Howl(o);");
        assert_eq!(scan.howl_sites.len(), 1);
    }

    #[test]
    fn an_object_passed_to_init_and_new_is_one_render_site() {
        let scan = scan("const o = {resolution: 2, autoDensity: !0}; a.init(o); new A(o);");
        assert_eq!(scan.render_configs.len(), 1);
    }

    #[test]
    fn a_legal_comment_feeds_the_engine_markers() {
        let scan = scan("/*! howler.js v2.2.4 */ f();");
        assert_eq!(
            scan.markers.howler_comment_version.as_deref(),
            Some("2.2.4")
        );
    }

    #[test]
    fn a_global_feeds_the_engine_markers() {
        assert!(scan("__sveltekit_abc.base;").markers.sveltekit_global);
    }

    #[test]
    fn a_local_variable_is_not_a_global_marker() {
        let scan = scan("let __sveltekit_abc = 1; __sveltekit_abc;");
        assert!(!scan.markers.sveltekit_global);
    }

    #[test]
    fn an_object_key_is_not_a_global_marker() {
        assert!(!scan("({__sveltekit_abc: 1});").markers.sveltekit_global);
    }

    #[test]
    fn an_object_key_can_be_a_howler_fingerprint() {
        assert!(scan("({html5PoolSize: 10});").markers.howler_fingerprint);
    }

    #[test]
    fn the_pixi_8_banner_version_is_read_through_its_variable() {
        let scan = scan(
            "var Ci; function l() { Ci = '8.22.0'; } console.log(`PixiJS ${Ci} - ${r} - http://www.pixijs.com/`);",
        );
        let banner = scan.markers.pixi_banner.unwrap();
        assert_eq!(banner.version.as_deref(), Some("8.22.0"));
        assert_eq!(banner.major, 8);
    }

    #[test]
    fn a_pixi_banner_version_gives_the_major_over_the_site() {
        let scan = scan("console.log(`PixiJS 8.1.0 - ${r} - https://pixijs.com`);");
        assert_eq!(scan.markers.pixi_banner.unwrap().major, 8);
    }

    #[test]
    fn a_pixi_banner_whose_version_cannot_be_read_keeps_its_major() {
        let scan = scan("console.log(`PixiJS ${q()} - ${r} - http://www.pixijs.com/`);");
        let banner = scan.markers.pixi_banner.unwrap();
        assert_eq!(banner.version, None);
        assert_eq!(banner.major, 8);
    }

    fn uses_a_canvas(source: &str) -> bool {
        scan_at(source, 0).unwrap().uses_a_canvas
    }

    #[test]
    fn a_canvas_context_method_call_on_any_object_uses_a_canvas() {
        assert!(uses_a_canvas("c.getContext('2d')"));
        assert!(uses_a_canvas(
            "document.querySelector('canvas').getContext('webgl2')"
        ));
        assert!(uses_a_canvas("c['getContext']('2d')"));
        assert!(uses_a_canvas("c.transferControlToOffscreen()"));
    }

    #[test]
    fn an_offscreen_canvas_uses_a_canvas() {
        assert!(uses_a_canvas("new OffscreenCanvas(1, 1)"));
        assert!(uses_a_canvas("new self.OffscreenCanvas(1, 1)"));
    }

    #[test]
    fn a_context_method_reached_another_way_uses_a_canvas() {
        assert!(uses_a_canvas("c.getContext.call(c, '2d')"));
        assert!(uses_a_canvas("const get = c.getContext.bind(c); get('2d')"));
        assert!(uses_a_canvas("HTMLCanvasElement.prototype.getContext"));
    }

    #[test]
    fn a_canvas_element_or_tag_uses_a_canvas() {
        assert!(uses_a_canvas("document.createElement('canvas')"));
        assert!(uses_a_canvas(
            "el.innerHTML = `<div><canvas id=${id}></canvas></div>`"
        ));
        assert!(uses_a_canvas("x instanceof HTMLCanvasElement"));
    }

    #[test]
    fn svelte_get_context_does_not_use_a_canvas() {
        assert!(!uses_a_canvas(
            "function g(k) { return getContext(k) } g('x')"
        ));
        assert!(!uses_a_canvas("q(state, `getContext`).get(key)"));
    }
}
