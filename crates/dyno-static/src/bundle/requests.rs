//! Request positions: where a script hands a string to something that fetches it (design
//! section 8.6). A whole URL anywhere else, such as in an error or log message, is not a
//! request.

use std::borrow::Cow;
use std::iter;

use oxc_ast::ast::{
    Argument, BinaryExpression, BinaryOperator, Expression, StaticMemberExpression, TemplateLiteral,
};

use crate::bundle::eval::{Evaluator, Property, Value};

/// A string a request position can be given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestText {
    /// The whole string is known.
    Whole(String),
    /// The text before the first part that is read at run time.
    Start {
        text: String,
        /// Where the template or concatenation starts in the script, so one built at run time
        /// counts once however many request positions reach it.
        site: u32,
    },
}

/// Follows one request position to the strings it can be given.
struct TextReader<'e, 's, 'a> {
    evaluator: &'e Evaluator<'s, 'a>,
    texts: Vec<RequestText>,
    steps: u32,
}

/// A part of a template or `a + b + ...` after its start.
#[derive(Clone, Copy)]
enum Part<'p, 'a> {
    /// Text written between a template's expressions, or None when it is not valid text.
    Text(Option<&'p str>),
    Expression(&'p Expression<'a>),
}

/// Functions, methods and constructors that request a URL passed as any argument, matched by
/// name: `fetch`, XHR and `window.open`, `navigator.sendBeacon`, `importScripts`, service
/// worker `register`, Pixi's `Assets.load` and `Texture.from`, and the constructors. Sorted.
const REQUEST_CALLS: &[&str] = &[
    "Audio",
    "EventSource",
    "Request",
    "SharedWorker",
    "URL",
    "WebSocket",
    "Worker",
    "fetch",
    "from",
    "importScripts",
    "load",
    "open",
    "register",
    "sendBeacon",
];

/// Object keys, assigned properties and attributes whose value gets requested, such as Howl's
/// `src` and `img.src`. Sorted.
const RESOURCE_KEYS: &[&str] = &["href", "poster", "src", "srcset", "url"];

/// `element.setAttribute(name, value)` requests `value` when `name` is a resource key.
const SET_ATTRIBUTE: &str = "setAttribute";

/// How deep and how far one request position is followed, as section 8.2 limits a value.
const MAX_DEPTH: u32 = 16;
const MAX_STEPS: u32 = 1_000;

/// Servers commonly refuse a request line past 8 KiB, so a longer text is not a URL a game
/// requests. The limit also bounds each text the reader copies.
const MAX_URL_BYTES: usize = 8 * 1_024;

/// The arguments that a call or `new` of `callee` requests.
pub fn requested_arguments<'c, 'a>(
    callee: &Expression<'a>,
    arguments: &'c [Argument<'a>],
    evaluator: &Evaluator<'_, 'a>,
) -> Vec<&'c Expression<'a>> {
    let mut expressions = arguments.iter().filter_map(Argument::as_expression);
    match callee_name(callee) {
        Some(name) if REQUEST_CALLS.contains(&name) => expressions.collect(),
        Some(SET_ATTRIBUTE) => {
            let attribute = expressions.next().and_then(|first| evaluator.value(first));
            // HTML attribute names ignore case.
            let sets_a_resource = attribute
                .as_ref()
                .and_then(Value::as_text)
                .is_some_and(|name| is_resource_key(&name.to_ascii_lowercase()));
            // The arguments after the name: the value, and any extra a browser ignores.
            if sets_a_resource {
                expressions.collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// Whether an object key or an assigned property named `name` holds a URL that gets
/// requested.
pub fn is_resource_key(name: &str) -> bool {
    RESOURCE_KEYS.contains(&name)
}

/// The strings a request position can be given, through parentheses, `?:` branches, logical
/// operands, the last expression of a comma list, array elements, the start of a concatenation,
/// variables written once and keys of object literals.
pub fn request_texts<'a>(
    evaluator: &Evaluator<'_, 'a>,
    expression: &Expression<'a>,
) -> Vec<RequestText> {
    let mut reader = TextReader {
        evaluator,
        texts: Vec::new(),
        steps: 0,
    };
    reader.read(expression, 0);
    reader.texts
}

/// The name of the function or method a callee calls.
fn callee_name<'c>(callee: &'c Expression<'_>) -> Option<&'c str> {
    match callee.without_parentheses() {
        Expression::Identifier(identifier) => Some(identifier.name.as_str()),
        Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
        // Minifiers write `(0, x.fetch)(url)` to call a method without its object.
        Expression::SequenceExpression(sequence) => callee_name(sequence.expressions.last()?),
        _ => None,
    }
}

impl<'a> TextReader<'_, '_, 'a> {
    fn read(&mut self, expression: &Expression<'a>, depth: u32) {
        if !self.step() || depth > MAX_DEPTH {
            return;
        }
        let deeper = depth + 1;
        match expression.without_parentheses() {
            Expression::StringLiteral(literal) => {
                if let Some(text) = literal.value.as_str() {
                    self.keep_whole(text);
                }
            }
            Expression::TemplateLiteral(template) => self.read_template(template, deeper),
            Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
                self.read_concatenation(binary, deeper);
            }
            Expression::ConditionalExpression(conditional) => {
                self.read(&conditional.consequent, deeper);
                self.read(&conditional.alternate, deeper);
            }
            Expression::LogicalExpression(logical) => {
                self.read(&logical.left, deeper);
                self.read(&logical.right, deeper);
            }
            Expression::SequenceExpression(sequence) => {
                if let Some(last) = sequence.expressions.last() {
                    self.read(last, deeper);
                }
            }
            Expression::ArrayExpression(array) => {
                for element in array.elements.iter().filter_map(|e| e.as_expression()) {
                    self.read(element, deeper);
                }
            }
            Expression::Identifier(identifier) => {
                if let Some(bound) = self.evaluator.bound_expression(identifier) {
                    self.read(bound, deeper);
                }
            }
            Expression::StaticMemberExpression(member) => self.read_key(member, deeper),
            _ => {}
        }
    }

    /// A template is read as its start, then each part joined in (see `join`).
    fn read_template(&mut self, template: &TemplateLiteral<'a>, depth: u32) {
        let leading = template.quasis.first().and_then(|quasi| quasi.value.cooked);
        let first = self.texts.len();
        let parts_in_the_start = match (
            leading.and_then(|text| text.as_str()),
            template.expressions.first(),
        ) {
            // `${a}/x` starts with what `a` reads as, as `a + '/x'` does.
            (Some(""), Some(start)) => {
                self.read(start, depth);
                2
            }
            (Some(text), _) => {
                self.keep_whole(text);
                1
            }
            (None, _) => return,
        };
        let parts = template_parts(template).skip(parts_in_the_start);
        self.join(first, parts, template.span.start, depth);
    }

    /// `a + b + ...` is read as its start `a`, then each part joined in (see `join`).
    fn read_concatenation(&mut self, binary: &BinaryExpression<'a>, depth: u32) {
        // A long `a + b + c` nests to the left, so it is walked in a loop, not by depth.
        let mut parts = vec![Part::Expression(&binary.right)];
        let mut start = &binary.left;
        while let Expression::BinaryExpression(inner) = start.without_parentheses()
            && inner.operator == BinaryOperator::Addition
        {
            if !self.step() {
                return;
            }
            parts.push(Part::Expression(&inner.right));
            start = &inner.left;
        }
        parts.reverse();
        let first = self.texts.len();
        self.read(start, depth);
        self.join(first, parts, binary.span.start, depth);
    }

    /// Joins each start read from `first` on with `parts`. A whole start stays whole while
    /// every part reads as one whole text; otherwise the text up to that part is only a start.
    fn join<'p>(
        &mut self,
        first: usize,
        parts: impl IntoIterator<Item = Part<'p, 'a>>,
        site: u32,
        depth: u32,
    ) where
        'a: 'p,
    {
        let starts = self.texts.split_off(first);
        let (suffix, every_part_is_known) = self.read_parts(parts, depth);
        for start in starts {
            let joined = match start {
                RequestText::Whole(left) if left.len() + suffix.len() > MAX_URL_BYTES => continue,
                RequestText::Whole(left) if every_part_is_known => {
                    RequestText::Whole(left + &suffix)
                }
                RequestText::Whole(left) => RequestText::Start {
                    text: left + &suffix,
                    site,
                },
                start @ RequestText::Start { .. } => start,
            };
            self.texts.push(joined);
        }
    }

    /// The texts of `parts` joined, and whether every part was read. Reading stops at the
    /// first part that is not one whole text, or that would make the text longer than a URL.
    fn read_parts<'p>(
        &mut self,
        parts: impl IntoIterator<Item = Part<'p, 'a>>,
        depth: u32,
    ) -> (String, bool)
    where
        'a: 'p,
    {
        let mut suffix = String::new();
        for part in parts {
            let text = match part {
                Part::Text(text) => text.map(Cow::Borrowed),
                Part::Expression(expression) => self.read_one(expression, depth).map(Cow::Owned),
            };
            match text {
                Some(text) if suffix.len() + text.len() <= MAX_URL_BYTES => suffix.push_str(&text),
                _ => return (suffix, false),
            }
        }
        (suffix, true)
    }

    /// The text `expression` reads as, when it reads as exactly one whole text.
    fn read_one(&mut self, expression: &Expression<'a>, depth: u32) -> Option<String> {
        let first = self.texts.len();
        self.read(expression, depth);
        match <[RequestText; 1]>::try_from(self.texts.split_off(first)) {
            Ok([RequestText::Whole(text)]) => Some(text),
            _ => None,
        }
    }

    /// `object.key`, where `object` is a variable bound to an object literal.
    fn read_key(&mut self, member: &StaticMemberExpression<'a>, depth: u32) {
        let Expression::Identifier(identifier) = member.object.without_parentheses() else {
            return;
        };
        let Some(keys) = self.evaluator.bound_keys(identifier) else {
            return;
        };
        if let Property::Written(value) = keys.get(&member.property.name) {
            self.read(value, depth);
        }
    }

    /// Keeps `text` as a whole text unless it is longer than a URL can be. The length is
    /// checked before the copy, as a script can hold a string of many megabytes.
    fn keep_whole(&mut self, text: &str) {
        if text.len() <= MAX_URL_BYTES {
            self.texts.push(RequestText::Whole(text.to_owned()));
        }
    }

    /// Counts one step, and says whether the position is still inside the step limit.
    fn step(&mut self) -> bool {
        self.steps = self.steps.saturating_add(1);
        self.steps <= MAX_STEPS
    }
}

/// A template's parts in order: the text before each expression, then the expression.
fn template_parts<'p, 'a>(template: &'p TemplateLiteral<'a>) -> impl Iterator<Item = Part<'p, 'a>> {
    template
        .quasis
        .iter()
        .enumerate()
        .flat_map(|(index, quasi)| {
            let text = Part::Text(quasi.value.cooked.and_then(|cooked| cooked.as_str()));
            iter::once(text).chain(template.expressions.get(index).map(Part::Expression))
        })
}

#[cfg(test)]
mod tests {
    use dyno_schema::path::RelPath;
    use dyno_schema::profile::HostProfile;

    use super::*;
    use crate::bundle::js::{ScriptAt, scan_script};

    fn url_count(source: &str) -> usize {
        let file = RelPath::new("main.js").unwrap();
        let at = ScriptAt {
            file: &file,
            offset: 0,
        };
        scan_script(at, source, &HostProfile::reels())
            .unwrap()
            .urls
            .len()
    }

    /// `fetch` of a variable that reaches a URL through `links` variables.
    fn chain_of_variables(links: u32) -> String {
        let mut source = String::from("const v0 = 'https://a.example.invalid/x';");
        for link in 1..=links {
            source.push_str(&format!("const v{link} = v{};", link - 1));
        }
        source.push_str(&format!("fetch(v{links});"));
        source
    }

    /// A whole URL of `length` bytes.
    fn url_of_length(length: usize) -> String {
        let start = "https://a.example.invalid/";
        format!("{start}{}", "x".repeat(length - start.len()))
    }

    /// `fetch` of a template whose URL ends only after `parts` empty expressions.
    fn template_of_parts(parts: u32) -> String {
        let empty = "${''}".repeat(parts as usize);
        format!("fetch(`https://a.example.invalid{empty}/x`);")
    }

    /// `fetch` of `a + b + ...` whose URL ends only after `parts` empty strings.
    fn chain_of_parts(parts: u32) -> String {
        let empty = " + ''".repeat(parts as usize);
        format!("fetch('https://a.example.invalid'{empty} + '/x');")
    }

    /// The most empty parts a chain can hold and still be read whole: the chain, each `+`
    /// inside it, its start and each part take one step.
    const CHAIN_PARTS_INSIDE_THE_STEP_LIMIT: u32 = (MAX_STEPS - 3) / 2;

    /// `fetch` of an array whose last element of `length` is a URL.
    fn array_ending_in_a_url(length: u32) -> String {
        let others = "'x',".repeat(length as usize - 1);
        format!("fetch([{others}'https://a.example.invalid/x']);")
    }

    #[test]
    fn a_chain_of_variables_inside_the_depth_limit_is_followed() {
        assert_eq!(url_count(&chain_of_variables(MAX_DEPTH - 1)), 1);
    }

    #[test]
    fn a_chain_of_variables_past_the_depth_limit_is_not_followed() {
        assert_eq!(url_count(&chain_of_variables(MAX_DEPTH)), 0);
    }

    #[test]
    fn circular_variables_end() {
        assert_eq!(url_count("var a = b, b = a; fetch(a);"), 0);
    }

    #[test]
    fn an_element_inside_the_step_limit_is_read() {
        assert_eq!(url_count(&array_ending_in_a_url(MAX_STEPS - 1)), 1);
    }

    #[test]
    fn an_element_past_the_step_limit_is_not_read() {
        assert_eq!(url_count(&array_ending_in_a_url(MAX_STEPS)), 0);
    }

    #[test]
    fn a_template_inside_the_step_limit_is_read_whole() {
        assert_eq!(url_count(&template_of_parts(MAX_STEPS - 1)), 1);
    }

    #[test]
    fn a_template_past_the_step_limit_keeps_only_its_start() {
        // The start `https://a.example.invalid` is a host still being built, not a URL.
        assert_eq!(url_count(&template_of_parts(MAX_STEPS)), 0);
    }

    #[test]
    fn a_chain_inside_the_step_limit_is_read_whole() {
        assert_eq!(
            url_count(&chain_of_parts(CHAIN_PARTS_INSIDE_THE_STEP_LIMIT)),
            1
        );
    }

    #[test]
    fn a_chain_past_the_step_limit_keeps_only_its_start() {
        let parts = CHAIN_PARTS_INSIDE_THE_STEP_LIMIT + 1;
        assert_eq!(url_count(&chain_of_parts(parts)), 0);
    }

    #[test]
    fn a_part_longer_than_a_url_can_be_ends_the_known_start() {
        let tail = format!("const t = '{}';", "x".repeat(MAX_URL_BYTES + 1));
        let start = "https://a.example.invalid/";
        assert_eq!(url_count(&format!("{tail} fetch('{start}' + t);")), 1);
        assert_eq!(url_count(&format!("{tail} fetch(`{start}${{t}}`);")), 1);
    }

    #[test]
    fn a_url_as_long_as_a_url_can_be_is_read() {
        assert_eq!(
            url_count(&format!("fetch('{}')", url_of_length(MAX_URL_BYTES))),
            1
        );
    }

    #[test]
    fn a_text_longer_than_a_url_can_be_is_not_read() {
        let url = url_of_length(MAX_URL_BYTES + 1);
        assert_eq!(url_count(&format!("fetch('{url}')")), 0);
    }

    #[test]
    fn a_join_longer_than_a_url_can_be_is_not_read() {
        let tail = format!("const t = '{}';", "x".repeat(MAX_URL_BYTES));
        let start = "https://a.example.invalid/";
        assert_eq!(url_count(&format!("{tail} fetch('{start}' + t);")), 0);
        assert_eq!(url_count(&format!("{tail} fetch(`{start}${{t}}`);")), 0);
    }
}
