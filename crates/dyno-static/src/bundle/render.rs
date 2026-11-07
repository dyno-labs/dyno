//! Render config sites: options objects that set up a Pixi renderer, and the least their canvas
//! holds (design sections 8.3 and 8.4).

use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
    UncertaintyKind,
};
use dyno_schema::path::RelPath;
use dyno_schema::profile::{HostProfile, Viewport};
use oxc_ast::ast::{AssignmentTarget, Expression};
use oxc_span::GetSpan;

use crate::bundle::eval::{Evaluator, NumberRange, ObjectKeys, Property, Value};
use crate::bundle::{estimated, exact};
use crate::model::{
    CanvasFloor, RESOLUTION_CAP_MILLI, RenderSite, Resolution, canvas_floor, resolution_text,
    saturating_i64,
};

/// The render config at one site, with where each value came from (design section 8.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderConfig {
    /// The script, or the HTML file of an inline script.
    pub file: RelPath,
    /// The byte offset of the options object from the start of `file`.
    pub offset: u32,
    pub callee: Callee,
    /// None when the value is not static.
    pub antialias: Option<bool>,
    pub antialias_from: OptionSource,
    pub resolution: Resolution,
    pub resolution_source: OptionSource,
    /// A note for people: the code the resolution came from.
    pub resolution_from: String,
    /// The backend the site asks for first. None when the value is not static.
    pub preference: Option<Backend>,
    /// The backend Pixi starts: the first one in the site's order that the profile has. None
    /// when the order is not static, or when the profile has none of them.
    pub modelled_as: Option<Backend>,
    /// None when the value is not static.
    pub auto_density: Option<bool>,
    pub size_from: SizeFrom,
    /// The CSS size the canvas floor used.
    pub canvas_css: Viewport,
    /// None when the profile has none of the backends the site allows, so Pixi fails to start.
    pub canvas: Option<CanvasFloor>,
}

/// How the options object reaches Pixi.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Callee {
    /// `new X({...})`, as Pixi 7 makes an Application.
    New,
    /// `app.init({...})`, as Pixi 8 starts an Application.
    Init,
    /// Any other call, such as `autoDetectRenderer({...})`.
    Call,
}

/// Where an option's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionSource {
    /// The evaluator reads the value from the code.
    Static,
    /// The key is absent, so Pixi's default applies.
    Default,
    /// The evaluator cannot read the value, or the key is absent and a script sets Pixi's
    /// default.
    NotStatic,
}

/// An option whose Pixi default a script can set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultOption {
    Antialias,
    Resolution,
}

/// The first code that sets Pixi's default for antialias and the resolution, as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefaultWrites {
    pub antialias: Option<String>,
    pub resolution: Option<String>,
}

/// A kind of Pixi renderer, as the `static.render.backend` measure names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Webgl,
    Webgpu,
    Canvas2d,
}

/// Where the canvas floor's CSS size came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeFrom {
    /// `resizeTo` makes the canvas follow an element or the window: the profile's frame.
    ResizeTo,
    /// A static `width` and `height`.
    WidthAndHeight,
    /// Neither `resizeTo` nor a static width and height is known, so the profile's frame is
    /// assumed.
    Assumed,
}

/// Where a site is and how it is called.
pub struct SiteAt<'f> {
    pub file: &'f RelPath,
    /// The byte offset of the options object from the start of `file`.
    pub offset: u32,
    pub callee: Callee,
}

/// The modelled backend's nested options, such as `webgl: {...}`.
enum Nested<'e, 'a> {
    Absent,
    /// The value is not static, so it may set any option.
    Hidden,
    Keys(ObjectKeys<'e, 'a>),
}

/// The options as Pixi's renderer sees them: the nested ones over the rest.
struct RendererOptions<'k, 'e, 's, 'a> {
    keys: &'k ObjectKeys<'e, 'a>,
    evaluator: &'k Evaluator<'s, 'a>,
    nested: Nested<'e, 'a>,
}

/// At least one of these keys makes an object a candidate (design section 8.3, rule 1).
const SIGNAL_KEYS: &[&str] = &[
    "antialias",
    "resolution",
    "preference",
    "autoDensity",
    "resizeTo",
    "webgl",
    "webgpu",
];

/// Every Application option of Pixi 7.4 and 8.x, from their published types (design section
/// 8.3, rule 2). Sorted.
const PIXI_OPTIONS: &[&str] = &[
    "accessibilityOptions",
    "antialias",
    "autoDensity",
    "autoStart",
    "background",
    "backgroundAlpha",
    "backgroundColor",
    "bezierSmoothness",
    "canvas",
    "canvasOptions",
    "clearBeforeRender",
    "context",
    "culler",
    "depth",
    "eventFeatures",
    "eventMode",
    "failIfMajorPerformanceCaveat",
    "forceCanvas",
    "forceFallbackAdapter",
    "gcActive",
    "gcFrequency",
    "gcMaxUnusedTime",
    "gpu",
    "height",
    "hello",
    "manageImports",
    "multiView",
    "powerPreference",
    "preferWebGLVersion",
    "preference",
    "premultipliedAlpha",
    "preserveDrawingBuffer",
    "renderableGCActive",
    "renderableGCFrequency",
    "renderableGCMaxUnusedTime",
    "resizeTo",
    "resolution",
    "roundPixels",
    "sharedTicker",
    "skipExtensionImports",
    "textureGCAMaxIdle",
    "textureGCActive",
    "textureGCCheckCountMax",
    "textureGCMaxIdle",
    "transparent",
    "useBackBuffer",
    "useContextAlpha",
    "view",
    "webgl",
    "webgpu",
    "width",
];

/// Keys only an Application takes, never a texture or render target (design section 8.3, rule
/// 3). Sorted.
const APPLICATION_ONLY_KEYS: &[&str] = &[
    "autoDensity",
    "autoStart",
    "background",
    "backgroundAlpha",
    "backgroundColor",
    "canvas",
    "clearBeforeRender",
    "failIfMajorPerformanceCaveat",
    "forceCanvas",
    "hello",
    "powerPreference",
    "preference",
    "premultipliedAlpha",
    "preserveDrawingBuffer",
    "resizeTo",
    "sharedTicker",
    "useBackBuffer",
    "view",
    "webgl",
    "webgpu",
];

/// Pixi 8's `app.init`.
const INIT_METHOD: &str = "init";

const ANTIALIAS: &str = "antialias";
const RESOLUTION: &str = "resolution";
const PREFERENCE: &str = "preference";
const FORCE_CANVAS: &str = "forceCanvas";
const AUTO_DENSITY: &str = "autoDensity";
const RESIZE_TO: &str = "resizeTo";
const WIDTH: &str = "width";
const HEIGHT: &str = "height";

// Pixi 7's and Pixi 8's defaults. Pixi 7's `settings.RESOLUTION` and Pixi 8's default
// resolution are both 1.
const DEFAULT_ANTIALIAS: bool = false;
const DEFAULT_AUTO_DENSITY: bool = false;
const DEFAULT_RESOLUTION_MILLI: u32 = 1_000;

// Writes that set a default: `X.defaultOptions.antialias = ...` in Pixi 7.2 and later,
// `settings.RENDER_OPTIONS.antialias = ...` in Pixi 7.0 and 7.1, and `settings.RESOLUTION = ...`.
const DEFAULT_HOLDERS: &[&str] = &["defaultOptions", "RENDER_OPTIONS"];
const SETTINGS_RESOLUTION: &str = "RESOLUTION";

// The keys of the options Pixi 8 merges over the rest for one backend.
const WEBGL_OPTIONS: &str = "webgl";
const WEBGPU_OPTIONS: &str = "webgpu";
const CANVAS_OPTIONS: &str = "canvasOptions";
const NESTED_OPTION_KEYS: [&str; 3] = [WEBGL_OPTIONS, WEBGPU_OPTIONS, CANVAS_OPTIONS];

/// Pixi 8's names for the backends in `preference`.
const PREFERENCE_NAMES: &[(&str, Backend)] = &[
    ("webgl", Backend::Webgl),
    ("webgpu", Backend::Webgpu),
    ("canvas", Backend::Canvas2d),
];

/// The order Pixi 8 tries backends in. A single preference goes first, then the rest in this
/// order.
const DEFAULT_ORDER: [Backend; 3] = [Backend::Webgl, Backend::Webgpu, Backend::Canvas2d];

const MILLI: f64 = 1_000.0;

const DEFAULT_NOTE: &str = "default";
const MAX_NOTE_CHARS: usize = 80;
const NOTE_ELLIPSIS: &str = "...";

const NO_RENDER_CONFIG: &str = "no render config found in the scripts";
const OPTION_NOT_STATIC: &str = "option_not_static";
const CANVAS2D: &str = "canvas2d";
const NO_AVAILABLE_BACKEND: &str = "no_available_backend";
const NO_CANVAS: &str = "a site with no canvas";
const NO_BACKEND: &str = "none";
const DRAWS_WITH_HTML: &str = "no canvas: the game draws with HTML and CSS";
const HOSTS_NOT_CHECKED: &str = "scripts from hosts built at run time are not checked";

const FLOOR_NOTE: &str = "one colour buffer, 4 bytes a backing pixel; antialias buffers and the GPU context are not \
     included";

/// The render measures, in `bundle::MEASURES` order.
const RENDER_MEASURES: [MeasureId; 4] = [
    MeasureId::StaticRenderAntialias,
    MeasureId::StaticRenderResolutionUncapped,
    MeasureId::StaticRenderBackend,
    MeasureId::StaticRenderCanvas,
];

impl Callee {
    /// A call's callee: `init` when it is a method named `init`.
    pub fn of_call(callee: &Expression<'_>) -> Callee {
        match callee.without_parentheses() {
            Expression::StaticMemberExpression(member) if member.property.name == INIT_METHOD => {
                Callee::Init
            }
            _ => Callee::Call,
        }
    }

    pub fn as_text(self) -> &'static str {
        match self {
            Callee::New => "new",
            Callee::Init => "init",
            Callee::Call => "call",
        }
    }
}

impl OptionSource {
    pub fn as_text(self) -> &'static str {
        match self {
            OptionSource::Static => "literal",
            OptionSource::Default => "default",
            OptionSource::NotStatic => OPTION_NOT_STATIC,
        }
    }
}

impl Backend {
    pub fn as_text(self) -> &'static str {
        match self {
            Backend::Webgl => "webgl",
            Backend::Webgpu => "webgpu",
            Backend::Canvas2d => "canvas2d",
        }
    }
}

impl SizeFrom {
    pub fn as_text(self) -> &'static str {
        match self {
            SizeFrom::ResizeTo => "resizeTo",
            SizeFrom::WidthAndHeight => "width and height",
            SizeFrom::Assumed => "assumed",
        }
    }
}

impl DefaultWrites {
    /// Keeps `code` when it is the first write of `option`'s default.
    pub fn add(&mut self, option: DefaultOption, code: &str) {
        let first = match option {
            DefaultOption::Antialias => &mut self.antialias,
            DefaultOption::Resolution => &mut self.resolution,
        };
        if first.is_none() {
            *first = Some(shorten(code));
        }
    }

    /// Keeps each write of `later` whose option has none yet.
    pub fn add_all(&mut self, later: DefaultWrites) {
        if self.antialias.is_none() {
            self.antialias = later.antialias;
        }
        if self.resolution.is_none() {
            self.resolution = later.resolution;
        }
    }
}

impl RenderConfig {
    /// For a build whose scripts set Pixi's defaults: an absent option takes a default that
    /// may be set before or after the site runs, so it is not static.
    pub fn follow_default_writes(&mut self, writes: &DefaultWrites) {
        if writes.antialias.is_some() && self.antialias_from == OptionSource::Default {
            self.antialias = None;
            self.antialias_from = OptionSource::NotStatic;
        }
        if let Some(write) = &writes.resolution
            && self.resolution_source == OptionSource::Default
        {
            self.resolution = Resolution::default();
            self.resolution_source = OptionSource::NotStatic;
            self.resolution_from = shorten(&format!("{DEFAULT_NOTE}: {write}"));
            if self.canvas.is_some() {
                self.canvas = Some(canvas_floor(self.canvas_css, self.resolution.floor_milli()));
            }
        }
    }

    /// Whether Pixi may start a GPU renderer here, where antialias matters.
    pub fn may_draw_on_the_gpu(&self) -> bool {
        self.modelled_as != Some(Backend::Canvas2d)
    }

    /// The row later steps read.
    pub fn site(&self) -> RenderSite {
        RenderSite {
            file: self.file.clone(),
            offset: self.offset,
            antialias: self.antialias,
            draws_in_2d: !self.may_draw_on_the_gpu(),
            resolution: self.resolution,
            canvas_css: self.canvas_css,
            canvas: self.canvas,
        }
    }
}

/// The render config at a site, when the keys of its options object pass the three rules of
/// design section 8.3. `text` is the script's text, for the resolution note.
pub fn read_config<'a>(
    at: SiteAt<'_>,
    keys: &ObjectKeys<'_, 'a>,
    evaluator: &Evaluator<'_, 'a>,
    profile: &HostProfile,
    text: &str,
) -> Option<RenderConfig> {
    if !is_render_site(keys, at.callee) {
        return None;
    }
    let order = backend_order(keys, evaluator);
    let modelled_as = order.as_ref().and_then(|order| {
        order
            .iter()
            .copied()
            .find(|backend| is_available(profile, *backend))
    });
    let nested = match order {
        Some(_) => nested_options(keys, evaluator, modelled_as),
        None => nested_options_of_any_backend(keys),
    };
    let options = RendererOptions {
        keys,
        evaluator,
        nested,
    };
    let (antialias, antialias_from) = options.antialias();
    let resolution_option = options.get(RESOLUTION);
    let resolution = match resolution_option {
        Property::Absent => Resolution::exactly(DEFAULT_RESOLUTION_MILLI),
        Property::Hidden => Resolution::default(),
        Property::Written(value) => resolution_of(evaluator, value),
    };
    let (canvas_css, size_from) = options.css_size(profile);
    // Every backend draws into a backing store, so only a Pixi that cannot start has no canvas.
    let starts = order.is_none() || modelled_as.is_some();
    let canvas = starts.then(|| canvas_floor(canvas_css, resolution.floor_milli()));
    Some(RenderConfig {
        file: at.file.clone(),
        offset: at.offset,
        callee: at.callee,
        antialias,
        antialias_from,
        resolution,
        resolution_source: source_of(resolution_option, resolution.fixed()),
        resolution_from: resolution_note(resolution_option, evaluator, text),
        preference: order.and_then(|order| order.first().copied()),
        modelled_as,
        auto_density: options.flag(AUTO_DENSITY, DEFAULT_AUTO_DENSITY),
        size_from,
        canvas_css,
        canvas,
    })
}

/// The option whose Pixi default an assignment to `target` sets.
pub fn default_set_by(target: &AssignmentTarget<'_>) -> Option<DefaultOption> {
    let member = target.as_member_expression()?;
    let name = member.static_property_name()?;
    if name == SETTINGS_RESOLUTION {
        return Some(DefaultOption::Resolution);
    }
    let holder = member
        .object()
        .as_member_expression()?
        .static_property_name()?;
    if !DEFAULT_HOLDERS
        .iter()
        .any(|default_holder| holder == *default_holder)
    {
        return None;
    }
    if name == ANTIALIAS {
        Some(DefaultOption::Antialias)
    } else if name == RESOLUTION {
        Some(DefaultOption::Resolution)
    } else {
        None
    }
}

/// The text of a syntax node in the script `text`.
pub fn source_text<'t>(node: &impl GetSpan, text: &'t str) -> &'t str {
    let span = node.span();
    text.get(span.start as usize..span.end as usize)
        .unwrap_or_default()
}

/// The four render measures, from every config sorted by file and offset (design section 2).
/// Each one says what the build proves, on its own. `draws_with_html` is true when the build
/// shows that the game has no canvas.
pub fn measures(
    configs: &[RenderConfig],
    draws_with_html: bool,
    not_covered: &[String],
) -> Vec<Measure> {
    let mut not_covered = not_covered.to_vec();
    let drawing: Vec<&RenderConfig> = configs
        .iter()
        .filter(|config| config.canvas.is_some())
        .collect();
    let mut measures = if configs.is_empty() && draws_with_html {
        // No canvas is not a free canvas: the browser run measures the page's layers.
        not_covered.push(HOSTS_NOT_CHECKED.to_owned());
        RENDER_MEASURES
            .iter()
            .map(|&id| match id {
                MeasureId::StaticRenderBackend => estimated(
                    id,
                    MeasureValue::Text(NO_BACKEND.into()),
                    Vec::new(),
                    exact(),
                ),
                _ => unsupported(id, DRAWS_WITH_HTML),
            })
            .collect()
    } else if configs.is_empty() {
        RENDER_MEASURES
            .iter()
            .map(|&id| Measure::missing(id, NO_RENDER_CONFIG))
            .collect()
    } else if drawing.is_empty() {
        RENDER_MEASURES
            .iter()
            .map(|&id| unsupported(id, NO_AVAILABLE_BACKEND))
            .collect()
    } else {
        // A site whose Pixi cannot start draws nothing, but the game may still fall back to it.
        not_covered.extend(configs.iter().filter_map(without_canvas));
        vec![
            antialias_measure(&drawing),
            resolution_measure(&drawing),
            backend_measure(&drawing),
            canvas_measure(&drawing),
        ]
    };
    for measure in &mut measures {
        measure.evidence = configs.iter().map(evidence).collect();
        measure.scope.not_covered.clone_from(&not_covered);
    }
    measures
}

/// The config with the largest canvas floor, with its floor. Of equal configs, the first wins.
pub fn worst_config(configs: &[RenderConfig]) -> Option<(&RenderConfig, CanvasFloor)> {
    let all: Vec<&RenderConfig> = configs.iter().collect();
    let worst = largest_floor(&all)?;
    Some((worst, worst.canvas?))
}

/// The three rules of design section 8.3.
fn is_render_site(keys: &ObjectKeys<'_, '_>, callee: Callee) -> bool {
    if keys.has_computed_key() {
        return false;
    }
    let names: Vec<&str> = keys.names().collect();
    let has_signal = names.iter().any(|name| SIGNAL_KEYS.contains(name));
    let every_name_is_an_option = names.iter().all(|name| PIXI_OPTIONS.contains(name));
    let is_an_application = callee == Callee::Init
        || names
            .iter()
            .any(|name| APPLICATION_ONLY_KEYS.contains(name));
    has_signal && every_name_is_an_option && is_an_application
}

/// The backends Pixi tries, in order. None when the order is not static.
fn backend_order<'a>(
    keys: &ObjectKeys<'_, 'a>,
    evaluator: &Evaluator<'_, 'a>,
) -> Option<Vec<Backend>> {
    // Pixi 7's canvas build (pixi.js-legacy) draws in 2D when `forceCanvas` is set.
    match keys.get(FORCE_CANVAS) {
        Property::Absent => {}
        Property::Hidden => return None,
        Property::Written(value) => {
            if evaluator.value(value)?.is_truthy() {
                return Some(vec![Backend::Canvas2d]);
            }
        }
    }
    let preference = match keys.get(PREFERENCE) {
        Property::Absent => return Some(DEFAULT_ORDER.to_vec()),
        Property::Hidden => return None,
        Property::Written(value) => evaluator.value(value)?,
    };
    // Pixi reads a falsy preference as none.
    if !preference.is_truthy() {
        return Some(DEFAULT_ORDER.to_vec());
    }
    match preference {
        Value::Text(name) => {
            let first = backend_named(&name)?;
            let rest = DEFAULT_ORDER
                .into_iter()
                .filter(|backend| *backend != first);
            Some([first].into_iter().chain(rest).collect())
        }
        Value::Array(names) => {
            let order = names
                .iter()
                .map(|name| backend_named(name.as_text()?))
                .collect::<Option<Vec<Backend>>>()?;
            (!order.is_empty()).then_some(order)
        }
        _ => None,
    }
}

fn backend_named(name: &str) -> Option<Backend> {
    PREFERENCE_NAMES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, backend)| *backend)
}

/// Whether the profile's browser can give this backend.
fn is_available(profile: &HostProfile, backend: Backend) -> bool {
    match backend {
        Backend::Webgpu => profile.webgpu,
        Backend::Webgl | Backend::Canvas2d => true,
    }
}

/// The options Pixi 8 merges over the rest for the modelled backend: `webgl: {...}` for WebGL.
fn nested_options<'e, 'a>(
    keys: &ObjectKeys<'e, 'a>,
    evaluator: &Evaluator<'_, 'a>,
    modelled_as: Option<Backend>,
) -> Nested<'e, 'a> {
    let key = match modelled_as {
        Some(Backend::Webgl) => WEBGL_OPTIONS,
        Some(Backend::Webgpu) => WEBGPU_OPTIONS,
        Some(Backend::Canvas2d) => CANVAS_OPTIONS,
        None => return Nested::Absent,
    };
    let value = match keys.get(key) {
        Property::Absent => return Nested::Absent,
        Property::Hidden => return Nested::Hidden,
        Property::Written(value) => value,
    };
    if let Some(object) = evaluator.object(value) {
        return Nested::Keys(evaluator.keys(object));
    }
    // Spreading a falsy value adds no options.
    let is_falsy = evaluator
        .value(value)
        .is_some_and(|value| !value.is_truthy());
    if is_falsy {
        Nested::Absent
    } else {
        Nested::Hidden
    }
}

/// The nested options when the backend is not static. Any backend's may win, so they hide every
/// option unless all are absent.
fn nested_options_of_any_backend<'e, 'a>(keys: &ObjectKeys<'e, 'a>) -> Nested<'e, 'a> {
    let all_absent = NESTED_OPTION_KEYS
        .iter()
        .all(|key| matches!(keys.get(key), Property::Absent));
    if all_absent {
        Nested::Absent
    } else {
        Nested::Hidden
    }
}

impl<'e, 'a> RendererOptions<'_, 'e, '_, 'a> {
    fn get(&self, name: &str) -> Property<'e, 'a> {
        match &self.nested {
            Nested::Absent => self.keys.get(name),
            Nested::Hidden => Property::Hidden,
            Nested::Keys(nested) => match nested.get(name) {
                Property::Absent => self.keys.get(name),
                found => found,
            },
        }
    }

    fn antialias(&self) -> (Option<bool>, OptionSource) {
        let antialias = self.flag(ANTIALIAS, DEFAULT_ANTIALIAS);
        (antialias, source_of(self.get(ANTIALIAS), antialias))
    }

    /// A flag option, or `default` when it is absent. None when it is not static.
    fn flag(&self, name: &str, default: bool) -> Option<bool> {
        match self.get(name) {
            Property::Absent => Some(default),
            Property::Hidden => None,
            Property::Written(value) => Some(self.evaluator.value(value)?.is_truthy()),
        }
    }

    /// The CSS size: the profile's frame for `resizeTo`, else a static width and height, else the
    /// frame, assumed.
    fn css_size(&self, profile: &HostProfile) -> (Viewport, SizeFrom) {
        // Pixi's resize plugin reads `resizeTo` from the top level only, and a falsy one as none.
        let resizes = match self.keys.get(RESIZE_TO) {
            Property::Absent => false,
            Property::Hidden => return (profile.viewport, SizeFrom::Assumed),
            Property::Written(value) => self
                .evaluator
                .value(value)
                .is_none_or(|value| value.is_truthy()),
        };
        if resizes {
            return (profile.viewport, SizeFrom::ResizeTo);
        }
        match (self.css_pixels(WIDTH), self.css_pixels(HEIGHT)) {
            (Some(width), Some(height)) => (Viewport { width, height }, SizeFrom::WidthAndHeight),
            _ => (profile.viewport, SizeFrom::Assumed),
        }
    }

    /// A static, positive size in CSS pixels, rounded. None when it does not fit.
    fn css_pixels(&self, name: &str) -> Option<u32> {
        let Property::Written(value) = self.get(name) else {
            return None;
        };
        let number = self.evaluator.value(value)?.as_number()?;
        let rounded = number.round();
        (rounded >= 1.0 && rounded <= f64::from(u32::MAX)).then_some(rounded as u32)
    }
}

/// Where an option's value came from, given the value read.
fn source_of<T>(property: Property<'_, '_>, value: Option<T>) -> OptionSource {
    match (property, value) {
        (Property::Absent, _) => OptionSource::Default,
        (_, Some(_)) => OptionSource::Static,
        (_, None) => OptionSource::NotStatic,
    }
}

/// What the build proves about a written resolution: one value, else the range the evaluator
/// bounds it to. A low bound rounds down and a high bound rounds up, so the range only widens:
/// 1.5006 is 1.500 to 1.501, and only a whole number of thousandths is one value.
fn resolution_of<'a>(evaluator: &Evaluator<'_, 'a>, value: &Expression<'a>) -> Resolution {
    let exact = evaluator
        .value(value)
        .and_then(|value| value.as_number())
        .and_then(NumberRange::exactly);
    let Some(range) = exact.or_else(|| evaluator.number_range(value)) else {
        return Resolution::default();
    };
    // A NaN resolution leaves Pixi's canvas with no size the build can state.
    if range.may_be_nan {
        return Resolution::default();
    }
    Resolution {
        low_milli: range.low.and_then(|low| milli_bound((low * MILLI).floor())),
        high_milli: range
            .high
            .and_then(|high| milli_bound((high * MILLI).ceil())),
    }
}

/// A bound in milli-units, when it is a positive number that fits.
fn milli_bound(scaled: f64) -> Option<u32> {
    (scaled >= 1.0 && scaled <= f64::from(u32::MAX)).then_some(scaled as u32)
}

/// The code the resolution came from, followed through one binding.
fn resolution_note<'a>(
    resolution: Property<'_, 'a>,
    evaluator: &Evaluator<'_, 'a>,
    text: &str,
) -> String {
    let value = match resolution {
        Property::Absent => return DEFAULT_NOTE.to_owned(),
        Property::Hidden => return OPTION_NOT_STATIC.to_owned(),
        Property::Written(value) => value,
    };
    let bound = match value.without_parentheses() {
        Expression::Identifier(identifier) => evaluator.bound_expression(identifier),
        _ => None,
    };
    let written = source_text(value, text);
    let full = match bound {
        Some(bound) => format!("{written} = {}", source_text(bound, text)),
        None => written.to_owned(),
    };
    shorten(&full)
}

fn shorten(note: &str) -> String {
    if note.chars().count() <= MAX_NOTE_CHARS {
        return note.to_owned();
    }
    let kept: String = note.chars().take(MAX_NOTE_CHARS).collect();
    format!("{kept}{NOTE_ELLIPSIS}")
}

/// `static.render.antialias`: true when a site that may draw on the GPU proves it on. A site
/// that sets it at run time leaves it unknown, unless another site proves it on.
fn antialias_measure(drawing: &[&RenderConfig]) -> Measure {
    let id = MeasureId::StaticRenderAntialias;
    let on_the_gpu: Vec<&RenderConfig> = drawing
        .iter()
        .copied()
        .filter(|config| config.may_draw_on_the_gpu())
        .collect();
    if on_the_gpu.is_empty() {
        return unsupported(id, CANVAS2D);
    }
    let items = flag_items(&on_the_gpu, |config| config.antialias);
    let flags: Vec<Option<bool>> = on_the_gpu.iter().map(|config| config.antialias).collect();
    match proven_flag(&flags) {
        Some(antialias) => estimated(id, MeasureValue::Flag(antialias), items, exact()),
        None => unsupported(id, OPTION_NOT_STATIC),
    }
}

/// `static.render.resolution_uncapped`: true when a site proves its resolution above the cap.
fn resolution_measure(drawing: &[&RenderConfig]) -> Measure {
    let id = MeasureId::StaticRenderResolutionUncapped;
    let items = flag_items(drawing, is_uncapped);
    let flags: Vec<Option<bool>> = drawing.iter().map(|config| is_uncapped(config)).collect();
    match proven_flag(&flags) {
        Some(uncapped) => estimated(id, MeasureValue::Flag(uncapped), items, exact()),
        None => unsupported(id, OPTION_NOT_STATIC),
    }
}

/// `static.render.backend`: the backend the site with the largest canvas floor asks for.
fn backend_measure(drawing: &[&RenderConfig]) -> Measure {
    let id = MeasureId::StaticRenderBackend;
    let Some(worst) = largest_floor(drawing) else {
        return unsupported(id, NO_AVAILABLE_BACKEND);
    };
    let Some(preference) = worst.preference else {
        return unsupported(id, OPTION_NOT_STATIC);
    };
    let items = drawing
        .iter()
        .filter_map(|config| {
            let preference = config.preference?;
            Some(config_item(
                config,
                MeasureValue::Text(preference.as_text().into()),
            ))
        })
        .collect();
    estimated(
        id,
        MeasureValue::Text(preference.as_text().into()),
        items,
        exact(),
    )
}

/// `static.render.canvas`: the largest canvas floor. Canvases that live at once hold at least
/// the largest one.
fn canvas_measure(drawing: &[&RenderConfig]) -> Measure {
    let id = MeasureId::StaticRenderCanvas;
    let Some(worst) = largest_floor(drawing) else {
        return unsupported(id, NO_AVAILABLE_BACKEND);
    };
    let Some(floor) = worst.canvas else {
        return unsupported(id, NO_AVAILABLE_BACKEND);
    };
    let items = drawing
        .iter()
        .filter_map(|config| {
            let canvas = config.canvas?;
            Some(config_item(
                config,
                MeasureValue::Integer(saturating_i64(canvas.bytes)),
            ))
        })
        .collect();
    estimated(
        id,
        MeasureValue::Integer(saturating_i64(floor.bytes)),
        items,
        floor_uncertainty(worst),
    )
}

/// The config with the largest canvas floor, the first of equal ones.
fn largest_floor<'c>(drawing: &[&'c RenderConfig]) -> Option<&'c RenderConfig> {
    let mut worst: Option<&'c RenderConfig> = None;
    for &config in drawing {
        let Some(canvas) = config.canvas else {
            continue;
        };
        let is_larger = worst
            .and_then(|largest| largest.canvas)
            .is_none_or(|largest| canvas.bytes > largest.bytes);
        if is_larger {
            worst = Some(config);
        }
    }
    worst
}

/// A flag the build proves across sites: true when one site proves it, unknown when no site
/// proves it and one does not say, false when every site proves it false.
fn proven_flag(flags: &[Option<bool>]) -> Option<bool> {
    if flags.contains(&Some(true)) {
        Some(true)
    } else if flags.contains(&None) {
        None
    } else {
        Some(false)
    }
}

fn unsupported(id: MeasureId, reason: &str) -> Measure {
    Measure {
        reason: Some(reason.to_owned()),
        ..Measure::blank(id, MeasureStatus::Unsupported)
    }
}

/// The scope line for a config whose Pixi cannot start.
fn without_canvas(config: &RenderConfig) -> Option<String> {
    if config.canvas.is_some() {
        return None;
    }
    Some(format!(
        "{NO_CANVAS}: {NO_AVAILABLE_BACKEND} at byte {} of {}",
        config.offset, config.file
    ))
}

/// One item for each config where `read` gives a flag.
fn flag_items(
    configs: &[&RenderConfig],
    read: fn(&RenderConfig) -> Option<bool>,
) -> Vec<MeasureItem> {
    configs
        .iter()
        .filter_map(|config| Some(config_item(config, MeasureValue::Flag(read(config)?))))
        .collect()
}

/// Whether the resolution is above the cap a fix sets. None when the build does not say.
fn is_uncapped(config: &RenderConfig) -> Option<bool> {
    config.resolution.is_above(RESOLUTION_CAP_MILLI)
}

fn config_item(config: &RenderConfig, value: MeasureValue) -> MeasureItem {
    MeasureItem {
        key: config.file.to_string(),
        value,
    }
}

/// The canvas is a floor: one colour buffer. The note says which of its inputs the build does
/// not fix.
fn floor_uncertainty(worst: &RenderConfig) -> Uncertainty {
    let mut note = FLOOR_NOTE.to_owned();
    if worst.resolution.floor_is_assumed() {
        let floor = worst.resolution.floor_milli();
        note.push_str(&format!(
            "; the resolution is set at run time, so the floor takes {}",
            resolution_text(floor)
        ));
    }
    if worst.size_from == SizeFrom::Assumed {
        note.push_str("; the size is set at run time, so the floor takes the profile's frame");
    }
    Uncertainty {
        kind: UncertaintyKind::Floor,
        note: Some(note),
        ..Uncertainty::unknown()
    }
}

fn evidence(config: &RenderConfig) -> Evidence {
    Evidence {
        method: Method::BundleScan,
        detail: format!(
            "{} options at byte {} of {}",
            config.callee.as_text(),
            config.offset,
            config.file
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::js::{ScriptAt, scan_script};

    fn configs_with(source: &str, profile: &HostProfile) -> Vec<RenderConfig> {
        let file = RelPath::new("main.js").unwrap();
        let at = ScriptAt {
            file: &file,
            offset: 0,
        };
        scan_script(at, source, profile).unwrap().render_configs
    }

    fn configs(source: &str) -> Vec<RenderConfig> {
        configs_with(source, &HostProfile::reels())
    }

    fn only_with(source: &str, profile: &HostProfile) -> RenderConfig {
        let mut configs = configs_with(source, profile);
        assert_eq!(configs.len(), 1, "{source}");
        configs.remove(0)
    }

    fn only(source: &str) -> RenderConfig {
        only_with(source, &HostProfile::reels())
    }

    fn profile_with_webgpu(webgpu: bool) -> HostProfile {
        HostProfile {
            webgpu,
            ..HostProfile::reels()
        }
    }

    fn measure_of(measures: &[Measure], id: MeasureId) -> &Measure {
        measures.iter().find(|measure| measure.id == id).unwrap()
    }

    fn measures_of(source: &str) -> Vec<Measure> {
        measures(&configs(source), false, &[])
    }

    fn assert_every_render_measure(measures: &[Measure], status: MeasureStatus, reason: &str) {
        for id in RENDER_MEASURES {
            let measure = measure_of(measures, id);
            assert_eq!(measure.status, status, "{id:?}");
            assert_eq!(measure.reason.as_deref(), Some(reason), "{id:?}");
        }
    }

    fn resolution_write(code: &str) -> DefaultWrites {
        DefaultWrites {
            resolution: Some(code.to_owned()),
            ..DefaultWrites::default()
        }
    }

    #[test]
    fn a_pixi_7_application_is_a_site_by_its_application_only_keys() {
        assert_eq!(
            only("new A({antialias: !0, autoDensity: !0})").callee,
            Callee::New
        );
    }

    #[test]
    fn a_pixi_8_init_call_is_a_site_with_signal_keys_alone() {
        let config = only("app.init({resolution: 2})");
        assert_eq!(config.callee, Callee::Init);
        assert_eq!(config.resolution.fixed(), Some(2_000));
        assert_eq!(config.resolution_source, OptionSource::Static);
    }

    #[test]
    fn the_device_pixel_ratio_is_the_profiles() {
        let config = only("app.init({resolution: devicePixelRatio})");
        assert_eq!(config.resolution.fixed(), Some(3_000));
    }

    #[test]
    fn auto_density_is_read() {
        assert_eq!(only("app.init({autoDensity: !0})").auto_density, Some(true));
    }

    #[test]
    fn resize_to_follows_the_profiles_frame() {
        let config = only("app.init({resizeTo: window, width: 720, height: 1280})");
        assert_eq!(config.size_from, SizeFrom::ResizeTo);
        assert_eq!(config.canvas_css, HostProfile::reels().viewport);
    }

    #[test]
    fn absent_options_take_pixis_defaults() {
        let config = only("app.init({resizeTo: window})");
        assert_eq!(config.antialias, Some(false));
        assert_eq!(config.antialias_from, OptionSource::Default);
        assert_eq!(config.resolution.fixed(), Some(1_000));
        assert_eq!(config.resolution_source, OptionSource::Default);
        assert_eq!(config.auto_density, Some(false));
        assert_eq!(config.preference, Some(Backend::Webgl));
    }

    #[test]
    fn antialias_is_read_as_javascript_truthiness() {
        let config = only("app.init({antialias: 1})");
        assert_eq!(config.antialias, Some(true));
        assert_eq!(config.antialias_from, OptionSource::Static);
    }

    #[test]
    fn a_texture_with_application_option_names_is_not_a_site() {
        assert_eq!(
            configs("new T({width: 64, height: 64, resolution: 2, antialias: !0})"),
            []
        );
    }

    #[test]
    fn an_object_with_a_key_no_application_takes_is_not_a_site() {
        let source = "r.initFromOptions({alpha: !0, antialias: !0, premultipliedAlpha: !0})";
        assert_eq!(configs(source), []);
    }

    #[test]
    fn an_object_without_a_signal_key_is_not_a_site() {
        assert_eq!(configs("app.init({background: '#000'})"), []);
    }

    #[test]
    fn an_object_with_a_computed_key_is_not_a_site() {
        assert_eq!(configs("app.init({resolution: 1, [k]: 2})"), []);
    }

    #[test]
    fn an_options_object_bound_to_a_variable_is_read_through_it() {
        let config = only(
            "var o = {resolution: devicePixelRatio || 1, width: 720, height: 1280, backgroundAlpha: 0}; new A(o);",
        );
        assert_eq!(config.offset, 8);
        assert_eq!(config.size_from, SizeFrom::WidthAndHeight);
        assert_eq!(
            config.canvas_css,
            Viewport {
                width: 720,
                height: 1280
            }
        );
    }

    #[test]
    fn one_object_passed_to_two_calls_is_one_site() {
        assert_eq!(
            configs("const o = {resolution: 2}; a.init(o); b.init(o);").len(),
            1
        );
    }

    #[test]
    fn an_option_the_evaluator_cannot_read_is_not_static() {
        let config = only("app.init({antialias: q(), resolution: r.resolution})");
        assert_eq!(config.antialias, None);
        assert_eq!(config.antialias_from, OptionSource::NotStatic);
        assert_eq!(config.resolution.fixed(), None);
        assert_eq!(config.resolution_source, OptionSource::NotStatic);
    }

    #[test]
    fn a_resolution_set_at_run_time_takes_the_floor_at_resolution_1() {
        let config = only("app.init({resolution: r.resolution})");
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 1_000)));
    }

    #[test]
    fn antialias_set_at_run_time_leaves_the_canvas_floor() {
        let config = only("app.init({antialias: q(), resolution: 2})");
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 2_000)));
    }

    #[test]
    fn a_resolution_helper_with_one_return_is_bounded() {
        let config = only(
            "function f(e, t) { return Math.min(e || 1, t ? 1.5 : 2) } \
             app.init({resolution: f(window.devicePixelRatio, q())})",
        );
        assert_eq!(
            config.resolution,
            Resolution {
                low_milli: Some(1_500),
                high_milli: Some(2_000),
            }
        );
        assert_eq!(config.resolution_source, OptionSource::NotStatic);
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 1_500)));
    }

    #[test]
    fn a_range_bound_rounds_outward() {
        let config = only("app.init({resolution: q() ? 1.0004 : 1.9996})");
        assert_eq!(
            config.resolution,
            Resolution {
                low_milli: Some(1_000),
                high_milli: Some(2_000),
            }
        );
    }

    #[test]
    fn a_resolution_between_two_thousandths_is_a_range_of_both() {
        let config = only("app.init({resolution: 2.0004})");
        assert_eq!(
            config.resolution,
            Resolution {
                low_milli: Some(2_000),
                high_milli: Some(2_001),
            }
        );
        assert_eq!(config.resolution.is_above(RESOLUTION_CAP_MILLI), None);
    }

    #[test]
    fn a_resolution_that_may_be_nan_is_not_static() {
        let config = only("app.init({resolution: Math.min(r.ratio, 2)})");
        assert_eq!(config.resolution, Resolution::default());
    }

    #[test]
    fn a_resolution_too_large_to_fit_is_not_static() {
        assert_eq!(
            only("app.init({resolution: 1e10})").resolution.fixed(),
            None
        );
    }

    #[test]
    fn a_set_default_resolution_makes_an_absent_resolution_not_static() {
        let mut config = only("app.init({antialias: !0})");
        let writes = resolution_write("PIXI.settings.RESOLUTION = devicePixelRatio");
        config.follow_default_writes(&writes);
        assert_eq!(config.resolution.fixed(), None);
        assert_eq!(config.resolution_source, OptionSource::NotStatic);
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 1_000)));
    }

    #[test]
    fn a_set_default_resolution_is_in_the_note() {
        let mut config = only("app.init({antialias: !0})");
        let writes = resolution_write("PIXI.settings.RESOLUTION = devicePixelRatio");
        config.follow_default_writes(&writes);
        assert_eq!(
            config.resolution_from,
            "default: PIXI.settings.RESOLUTION = devicePixelRatio"
        );
    }

    #[test]
    fn a_set_default_resolution_leaves_a_written_resolution() {
        let mut config = only("app.init({resolution: 2})");
        config.follow_default_writes(&resolution_write("s.RESOLUTION = 3"));
        assert_eq!(config.resolution.fixed(), Some(2_000));
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 2_000)));
    }

    #[test]
    fn a_set_default_antialias_leaves_a_written_antialias() {
        let mut config = only("app.init({antialias: !1, resolution: 2})");
        let writes = DefaultWrites {
            antialias: Some("V.defaultOptions.antialias = !0".to_owned()),
            ..DefaultWrites::default()
        };
        config.follow_default_writes(&writes);
        assert_eq!(config.antialias, Some(false));
    }

    #[test]
    fn a_set_default_antialias_makes_an_absent_antialias_not_static() {
        let mut config = only("app.init({resolution: 2})");
        let writes = DefaultWrites {
            antialias: Some("V.defaultOptions.antialias = !0".to_owned()),
            ..DefaultWrites::default()
        };
        config.follow_default_writes(&writes);
        assert_eq!(config.antialias, None);
        assert_eq!(config.antialias_from, OptionSource::NotStatic);
    }

    #[test]
    fn the_first_write_of_a_default_is_kept() {
        let mut writes = DefaultWrites::default();
        writes.add(DefaultOption::Resolution, "a.RESOLUTION = 1");
        writes.add(DefaultOption::Resolution, "b.RESOLUTION = 2");
        assert_eq!(writes.resolution.as_deref(), Some("a.RESOLUTION = 1"));
    }

    #[test]
    fn a_later_script_adds_only_the_defaults_not_yet_written() {
        let mut writes = resolution_write("a.RESOLUTION = 1");
        writes.add_all(DefaultWrites {
            antialias: Some("b.defaultOptions.antialias = !0".to_owned()),
            resolution: Some("c.RESOLUTION = 3".to_owned()),
        });
        assert_eq!(writes.resolution.as_deref(), Some("a.RESOLUTION = 1"));
        assert_eq!(
            writes.antialias.as_deref(),
            Some("b.defaultOptions.antialias = !0")
        );
    }

    #[test]
    fn a_spread_that_cannot_be_read_makes_the_options_not_static() {
        // The spread may set `antialias`, or `webgl` options that win over the rest.
        let config = only("app.init({antialias: !0, ...options, resolution: 2})");
        assert_eq!(config.antialias, None);
        assert_eq!(config.resolution.fixed(), None);
    }

    #[test]
    fn the_nested_options_of_the_modelled_backend_win() {
        let config =
            only("app.init({antialias: !1, resolution: 1, webgl: {antialias: !0, resolution: 2}})");
        assert_eq!(config.antialias, Some(true));
        assert_eq!(config.resolution.fixed(), Some(2_000));
    }

    #[test]
    fn the_nested_options_of_another_backend_are_ignored() {
        let source = "app.init({preference: 'webgpu', antialias: !1, webgpu: {antialias: !0}})";
        let config = only_with(source, &profile_with_webgpu(false));
        assert_eq!(config.modelled_as, Some(Backend::Webgl));
        assert_eq!(config.antialias, Some(false));
    }

    #[test]
    fn a_falsy_nested_options_value_adds_no_options() {
        assert_eq!(
            only("app.init({antialias: !0, webgl: !1})").antialias,
            Some(true)
        );
    }

    #[test]
    fn nested_options_the_evaluator_cannot_read_hide_every_option() {
        let config = only("app.init({antialias: !0, resolution: 1, webgl: gl.options})");
        assert_eq!(config.antialias, None);
        assert_eq!(config.resolution.fixed(), None);
    }

    #[test]
    fn a_computed_key_in_the_nested_options_hides_every_option() {
        let config = only("app.init({antialias: !0, resolution: 1, webgl: {[k]: !1}})");
        assert_eq!(config.antialias, None);
        assert_eq!(config.resolution.fixed(), None);
    }

    #[test]
    fn webgpu_is_modelled_as_webgl_when_the_profile_has_no_webgpu() {
        let source = "app.init({preference: 'webgpu', resolution: 1})";
        let config = only_with(source, &profile_with_webgpu(false));
        assert_eq!(config.preference, Some(Backend::Webgpu));
        assert_eq!(config.modelled_as, Some(Backend::Webgl));
    }

    #[test]
    fn webgpu_has_a_canvas_floor_when_the_profile_has_webgpu() {
        let source = "app.init({preference: 'webgpu', resolution: 1})";
        let config = only_with(source, &profile_with_webgpu(true));
        assert_eq!(config.modelled_as, Some(Backend::Webgpu));
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 1_000)));
    }

    #[test]
    fn a_preference_list_falls_back_in_its_own_order() {
        let config = only("app.init({preference: ['webgpu', 'canvas'], resolution: 1})");
        assert_eq!(config.preference, Some(Backend::Webgpu));
        assert_eq!(config.modelled_as, Some(Backend::Canvas2d));
        assert!(!config.may_draw_on_the_gpu());
    }

    #[test]
    fn a_preference_list_the_profile_cannot_meet_models_no_backend() {
        let source = "app.init({preference: ['webgpu'], resolution: 1})";
        let config = only_with(source, &profile_with_webgpu(false));
        assert_eq!(config.modelled_as, None);
        assert_eq!(config.canvas, None);
    }

    #[test]
    fn force_canvas_asks_for_a_2d_canvas() {
        let config = only("new A({forceCanvas: !0, resolution: 1})");
        assert_eq!(config.preference, Some(Backend::Canvas2d));
        assert_eq!(config.modelled_as, Some(Backend::Canvas2d));
    }

    #[test]
    fn a_2d_canvas_has_a_floor_like_any_other() {
        let config = only("new A({forceCanvas: !0, resolution: 2})");
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 2_000)));
    }

    #[test]
    fn a_preference_that_is_not_static_models_no_backend() {
        let config = only("app.init({preference: p, resolution: 1})");
        assert_eq!(config.preference, None);
        assert_eq!(config.modelled_as, None);
    }

    #[test]
    fn a_preference_that_is_not_static_keeps_the_canvas_floor() {
        let config = only("app.init({preference: p, resolution: 2})");
        assert_eq!(config.canvas, Some(canvas_floor(config.canvas_css, 2_000)));
        assert!(config.may_draw_on_the_gpu());
    }

    #[test]
    fn the_nested_options_of_any_backend_hide_an_option_when_the_preference_is_not_static() {
        let config =
            only("app.init({preference: p, resolution: 1, canvasOptions: {resolution: 2}})");
        assert_eq!(config.resolution.fixed(), None);
    }

    #[test]
    fn without_nested_options_a_top_level_option_is_read_when_the_preference_is_not_static() {
        let config = only("app.init({preference: p, resolution: 2})");
        assert_eq!(config.resolution.fixed(), Some(2_000));
    }

    #[test]
    fn a_force_canvas_that_is_not_static_may_draw_on_the_gpu() {
        let config = only("new A({forceCanvas: f, resolution: 1})");
        assert_eq!(config.modelled_as, None);
        assert!(config.may_draw_on_the_gpu());
    }

    #[test]
    fn a_falsy_resize_to_does_not_resize() {
        let config = only("app.init({resizeTo: !1, width: 720, height: 1280})");
        assert_eq!(config.size_from, SizeFrom::WidthAndHeight);
    }

    #[test]
    fn a_nested_resize_to_does_not_resize() {
        let config = only("app.init({width: 720, height: 1280, webgl: {resizeTo: window}})");
        assert_eq!(config.size_from, SizeFrom::WidthAndHeight);
    }

    #[test]
    fn a_spread_that_may_set_resize_to_assumes_the_frame() {
        let config = only("app.init({width: 720, height: 1280, ...o, resolution: 1})");
        assert_eq!(config.size_from, SizeFrom::Assumed);
        assert_eq!(config.canvas_css, HostProfile::reels().viewport);
    }

    #[test]
    fn without_resize_to_or_a_static_size_the_frame_is_assumed() {
        let config = only("app.init({resolution: 1, width: w, height: 600})");
        assert_eq!(config.size_from, SizeFrom::Assumed);
        assert_eq!(config.canvas_css, HostProfile::reels().viewport);
    }

    #[test]
    fn the_resolution_note_shows_the_code_through_one_binding() {
        let config = only("const r = Math.min(devicePixelRatio, 2); app.init({resolution: r})");
        assert_eq!(config.resolution_from, "r = Math.min(devicePixelRatio, 2)");
    }

    #[test]
    fn an_absent_resolution_has_the_default_note() {
        assert_eq!(only("app.init({antialias: !0})").resolution_from, "default");
    }

    #[test]
    fn a_long_resolution_note_is_shortened() {
        let resolution = format!("Math.min(devicePixelRatio, {})", "1 || ".repeat(20) + "2");
        let config = only(&format!("app.init({{resolution: {resolution}}})"));
        assert_eq!(
            config.resolution_from.chars().count(),
            MAX_NOTE_CHARS + NOTE_ELLIPSIS.chars().count()
        );
        assert!(config.resolution_from.ends_with(NOTE_ELLIPSIS));
    }

    #[test]
    fn the_canvas_and_backend_come_from_the_config_with_the_largest_floor() {
        let configs = configs(
            "a.init({resolution: 1}); b.init({resolution: 2, preference: 'webgpu', antialias: !0});",
        );
        let measures = measures(&configs, false, &[]);
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(
            canvas.value,
            Some(MeasureValue::Integer(saturating_i64(
                configs[1].canvas.unwrap().bytes
            )))
        );
        assert_eq!(canvas.items.len(), 2);
        let backend = measure_of(&measures, MeasureId::StaticRenderBackend);
        assert_eq!(backend.value, Some(MeasureValue::Text("webgpu".into())));
    }

    #[test]
    fn of_configs_with_equal_floors_the_first_is_the_worst() {
        let configs = configs("a.init({resolution: 1}); b.init({resolution: 1, antialias: !1});");
        let (worst, _) = worst_config(&configs).unwrap();
        assert_eq!(worst.offset, configs[0].offset);
    }

    #[test]
    fn antialias_is_on_when_any_site_proves_it() {
        let measures = measures_of("a.init({antialias: q()}); b.init({antialias: !0});");
        let antialias = measure_of(&measures, MeasureId::StaticRenderAntialias);
        assert_eq!(antialias.value, Some(MeasureValue::Flag(true)));
        assert_eq!(antialias.items.len(), 1);
    }

    #[test]
    fn antialias_is_unknown_when_no_site_proves_it_on_and_one_sets_it_at_run_time() {
        let measures = measures_of("a.init({antialias: q()}); b.init({antialias: !1});");
        let antialias = measure_of(&measures, MeasureId::StaticRenderAntialias);
        assert_eq!(antialias.status, MeasureStatus::Unsupported);
        assert_eq!(antialias.reason.as_deref(), Some(OPTION_NOT_STATIC));
    }

    #[test]
    fn antialias_is_off_when_every_site_proves_it_off() {
        let measures = measures_of("a.init({antialias: !1}); b.init({resolution: 2});");
        let antialias = measure_of(&measures, MeasureId::StaticRenderAntialias);
        assert_eq!(antialias.value, Some(MeasureValue::Flag(false)));
    }

    #[test]
    fn a_2d_site_does_not_count_toward_antialias() {
        let measures =
            measures_of("a.init({antialias: !1}); new B({forceCanvas: !0, antialias: q()})");
        let antialias = measure_of(&measures, MeasureId::StaticRenderAntialias);
        assert_eq!(antialias.value, Some(MeasureValue::Flag(false)));
    }

    #[test]
    fn antialias_is_unsupported_when_every_site_is_2d() {
        let measures = measures_of("new A({forceCanvas: !0, antialias: !0, resolution: 1})");
        let antialias = measure_of(&measures, MeasureId::StaticRenderAntialias);
        assert_eq!(antialias.status, MeasureStatus::Unsupported);
        assert_eq!(antialias.reason.as_deref(), Some(CANVAS2D));
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.status, MeasureStatus::Estimated);
    }

    #[test]
    fn the_resolution_is_uncapped_when_any_site_proves_it() {
        let measures = measures_of("a.init({resolution: r()}); b.init({resolution: 3});");
        let uncapped = measure_of(&measures, MeasureId::StaticRenderResolutionUncapped);
        assert_eq!(uncapped.value, Some(MeasureValue::Flag(true)));
    }

    #[test]
    fn the_resolution_is_unknown_when_a_site_sets_it_at_run_time() {
        let measures = measures_of("a.init({resolution: r()}); b.init({resolution: 2});");
        let uncapped = measure_of(&measures, MeasureId::StaticRenderResolutionUncapped);
        assert_eq!(uncapped.status, MeasureStatus::Unsupported);
        assert_eq!(uncapped.reason.as_deref(), Some(OPTION_NOT_STATIC));
    }

    #[test]
    fn the_backend_is_unknown_when_the_preference_is_not_static() {
        let measures = measures_of("app.init({preference: p, resolution: 1})");
        let backend = measure_of(&measures, MeasureId::StaticRenderBackend);
        assert_eq!(backend.status, MeasureStatus::Unsupported);
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.status, MeasureStatus::Estimated);
    }

    #[test]
    fn every_render_measure_is_unsupported_when_no_site_can_start() {
        let profile = profile_with_webgpu(false);
        let source = "a.init({preference: ['webgpu'], resolution: 1})";
        let measures = measures(&configs_with(source, &profile), false, &[]);
        assert_every_render_measure(&measures, MeasureStatus::Unsupported, NO_AVAILABLE_BACKEND);
    }

    #[test]
    fn a_site_that_cannot_start_next_to_one_that_can_is_not_covered() {
        let source = "a.init({resolution: 1}); b.init({preference: ['webgpu'], resolution: 1})";
        let measures = measures(&configs(source), false, &[]);
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.status, MeasureStatus::Estimated);
        assert_eq!(
            canvas.scope.not_covered,
            ["a site with no canvas: no_available_backend at byte 32 of main.js"]
        );
    }

    #[test]
    fn the_measures_are_missing_without_a_site() {
        let measures = measures(&[], false, &[]);
        assert_every_render_measure(&measures, MeasureStatus::Missing, NO_RENDER_CONFIG);
    }

    #[test]
    fn a_game_that_draws_with_html_has_no_backend_and_no_canvas_value() {
        let measures = measures(&[], true, &[]);
        let backend = measure_of(&measures, MeasureId::StaticRenderBackend);
        assert_eq!(backend.value, Some(MeasureValue::Text("none".into())));
        for id in [
            MeasureId::StaticRenderAntialias,
            MeasureId::StaticRenderResolutionUncapped,
            MeasureId::StaticRenderCanvas,
        ] {
            let measure = measure_of(&measures, id);
            assert_eq!(measure.status, MeasureStatus::Unsupported, "{id:?}");
            assert_eq!(measure.reason.as_deref(), Some(DRAWS_WITH_HTML), "{id:?}");
        }
        assert_eq!(backend.scope.not_covered, [HOSTS_NOT_CHECKED]);
    }

    #[test]
    fn a_site_wins_over_drawing_with_html() {
        let measures = measures(&configs("app.init({resolution: 2})"), true, &[]);
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.status, MeasureStatus::Estimated);
    }

    #[test]
    fn the_canvas_is_a_floor_of_one_colour_buffer() {
        let measures = measures_of("app.init({antialias: !0, resizeTo: window, resolution: 2})");
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(canvas.uncertainty.kind, UncertaintyKind::Floor);
        assert_eq!(canvas.uncertainty.note.as_deref(), Some(FLOOR_NOTE));
    }

    #[test]
    fn the_floor_note_says_when_the_resolution_and_size_are_set_at_run_time() {
        let measures = measures_of("app.init({resolution: r(), width: w, height: h})");
        let canvas = measure_of(&measures, MeasureId::StaticRenderCanvas);
        assert_eq!(
            canvas.uncertainty.note.as_deref(),
            Some(
                format!(
                    "{FLOOR_NOTE}; the resolution is set at run time, so the floor takes 1; the \
                     size is set at run time, so the floor takes the profile's frame"
                )
                .as_str()
            )
        );
    }
}
