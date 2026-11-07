//! A small evaluator for option values in minified code (design section 8.2).
//!
//! It reads literals, arrays, `!`, `-`, `+` and `void`, the globals `undefined`, `NaN`,
//! `Infinity` and `devicePixelRatio`, `Math.min` and `Math.max`, `||`, `&&`, `??`, comparisons,
//! conditionals, bindings written once, calls to a function whose body is one `return`, and
//! the keys of object literals. Anything else is not static, and the evaluator says so. A
//! binding to an object literal whose keys the script writes later, such as `o.x = 1` or
//! `Object.assign(o, more)`, is not followed.
//!
//! Where a number is not static, `number_range` can still bound it: `Math.min(x, 2)` is at most
//! 2, and `t ? 1.5 : 2` is 1.5 to 2 whatever `t` is.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    ArrayExpression, ArrowFunctionBody, AssignmentOperator, BinaryExpression, BinaryOperator,
    BindingPattern, CallExpression, ConditionalExpression, Expression, FormalParameters,
    FunctionBody, IdentifierReference, LogicalExpression, LogicalOperator, ObjectExpression,
    ObjectPropertyKind, Statement, StaticMemberExpression, UnaryExpression, UnaryOperator,
};
use oxc_semantic::{Semantic, SymbolId};
use oxc_span::{GetSpan, Span};

/// A value the evaluator worked out.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
    Undefined,
    Array(Vec<Value>),
}

/// A number known to lie from `low` to `high`, or NaN when `may_be_nan`. A side is None when
/// nothing bounds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberRange {
    pub low: Option<f64>,
    pub high: Option<f64>,
    /// `Math.min` of a value that is not a number is NaN, and `NaN || x` is `x`.
    pub may_be_nan: bool,
}

/// Reads the values of expressions in one parsed script.
pub struct Evaluator<'s, 'a> {
    semantic: &'s Semantic<'a>,
    device_scale_factor: f64,
    /// The steps taken for the value or keys being read now.
    steps: Cell<u32>,
    /// Each variable's one written expression, once looked up.
    bindings: RefCell<HashMap<SymbolId, Option<&'a Expression<'a>>>>,
    /// The keys of each variable bound to an object literal, once read.
    bound_keys: RefCell<HashMap<SymbolId, Option<Rc<ObjectKeys<'a, 'a>>>>>,
    /// The parameters of the functions being called now, each with what its argument gave.
    arguments: RefCell<HashMap<SymbolId, Argument>>,
    /// The functions being called now, so a function that calls itself is not followed.
    calling: RefCell<HashSet<SymbolId>>,
}

/// What the evaluator knows about one argument of a call it follows.
#[derive(Clone, Debug)]
struct Argument {
    value: Option<Value>,
    range: Option<NumberRange>,
}

/// A function whose body is one `return`: its parameters and the returned expression.
struct OneReturn<'a> {
    parameters: Vec<SymbolId>,
    returns: &'a Expression<'a>,
}

/// An object literal's keys once its spreads are read.
#[derive(Debug, Default)]
pub struct ObjectKeys<'e, 'a> {
    /// Each key's last value, with the number of the property that wrote it.
    values: BTreeMap<String, (usize, &'e Expression<'a>)>,
    /// The properties read so far, counting those of each spread that was read.
    properties: usize,
    /// A spread or computed key after this many properties may have replaced each value
    /// written so far.
    hidden_through: Option<usize>,
    /// A spread that is not an object literal may add keys that are not listed.
    has_unread_spread: bool,
    /// A key whose name is computed at run time may be any key.
    has_computed_key: bool,
}

/// What an object literal says about one key.
#[derive(Clone, Copy, Debug)]
pub enum Property<'e, 'a> {
    Absent,
    /// The key may be set, but a spread or a computed key hides its value.
    Hidden,
    Written(&'e Expression<'a>),
}

/// How deep the evaluator follows nested expressions, bindings and spreads. Option values are
/// short, so this only stops deep or circular code.
const MAX_DEPTH: u32 = 16;

/// How many expressions and spreads the evaluator reads for one value or one object's keys.
/// This stops bindings that fan out, such as arrays of arrays of arrays.
const MAX_STEPS: u32 = 1_000;

/// The globals that read the device pixel ratio as `<global>.devicePixelRatio`.
const PIXEL_RATIO_OWNERS: &[&str] = &["window", "self", "globalThis"];

const DEVICE_PIXEL_RATIO: &str = "devicePixelRatio";
const GLOBAL_UNDEFINED: &str = "undefined";
const GLOBAL_NAN: &str = "NaN";
const GLOBAL_INFINITY: &str = "Infinity";
const MATH: &str = "Math";
const MATH_MIN: &str = "min";
const MATH_MAX: &str = "max";

/// Calls that write keys into their first argument, as `Object.assign(o, more)` does.
const KEY_WRITING_CALLS: &[&str] = &["assign", "defineProperty", "defineProperties"];

impl Value {
    /// JavaScript's truthiness.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Number(number) => *number != 0.0 && !number.is_nan(),
            Value::Text(text) => !text.is_empty(),
            Value::Bool(flag) => *flag,
            Value::Null | Value::Undefined => false,
            Value::Array(_) => true,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(number) => Some(*number),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }
}

impl<'e, 'a> ObjectKeys<'e, 'a> {
    pub fn get(&self, key: &str) -> Property<'e, 'a> {
        match self.values.get(key) {
            Some(&(written_at, value)) if !self.is_hidden(written_at) => Property::Written(value),
            Some(_) => Property::Hidden,
            None if self.may_have_other_keys() => Property::Hidden,
            None => Property::Absent,
        }
    }

    /// Every key with a static name, including keys from spreads that were read.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    pub fn has_computed_key(&self) -> bool {
        self.has_computed_key
    }

    /// Whether a spread that was not read or a computed key may add keys that are not listed.
    pub fn may_have_other_keys(&self) -> bool {
        self.has_unread_spread || self.has_computed_key
    }

    fn write(&mut self, name: &str, value: &'e Expression<'a>) {
        self.properties += 1;
        self.values
            .insert(name.to_owned(), (self.properties, value));
    }

    /// Marks every value written so far as possibly replaced.
    fn hide_every_value(&mut self) {
        self.hidden_through = Some(self.properties);
    }

    fn is_hidden(&self, written_at: usize) -> bool {
        self.hidden_through
            .is_some_and(|hidden_through| written_at <= hidden_through)
    }
}

impl<'s, 'a> Evaluator<'s, 'a> {
    /// `device_scale_factor` is what `devicePixelRatio` reads on the modelled device.
    pub fn new(semantic: &'s Semantic<'a>, device_scale_factor: f64) -> Self {
        Evaluator {
            semantic,
            device_scale_factor,
            steps: Cell::new(0),
            bindings: RefCell::new(HashMap::new()),
            bound_keys: RefCell::new(HashMap::new()),
            arguments: RefCell::new(HashMap::new()),
            calling: RefCell::new(HashSet::new()),
        }
    }

    /// The value of `expression`, or None when it is not static.
    pub fn value(&self, expression: &Expression<'a>) -> Option<Value> {
        self.steps.set(0);
        self.value_at_depth(expression, 0)
    }

    /// The numbers `expression` can be, when the evaluator can bound them. A static number is a
    /// range of one value. None when nothing bounds it, or when it may not be a number.
    pub fn number_range(&self, expression: &Expression<'a>) -> Option<NumberRange> {
        self.steps.set(0);
        self.range_at_depth(expression, 0)
    }

    /// The object literal that `expression` is, or that the identifier `expression` is bound to.
    pub fn object<'e>(&self, expression: &'e Expression<'a>) -> Option<&'e ObjectExpression<'a>> {
        match expression.without_parentheses() {
            Expression::ObjectExpression(object) => Some(object),
            Expression::Identifier(identifier) => match self.bound_expression(identifier)? {
                Expression::ObjectExpression(object) => Some(object),
                _ => None,
            },
            _ => None,
        }
    }

    /// The keys of `object`, with each spread of an object literal (or a binding to one) read in
    /// place.
    pub fn keys<'e>(&self, object: &'e ObjectExpression<'a>) -> ObjectKeys<'e, 'a> {
        self.steps.set(0);
        let mut keys = ObjectKeys::default();
        self.add_keys(object, &mut keys, 0);
        keys
    }

    /// The one expression written to the variable that `identifier` names: its declarator's
    /// `init` or a single plain assignment. None for a global, a parameter, a function, a
    /// destructured binding, a variable never written, one written more than once, or an object
    /// literal whose keys the script writes later.
    pub fn bound_expression(
        &self,
        identifier: &IdentifierReference<'a>,
    ) -> Option<&'a Expression<'a>> {
        let symbol = self.symbol(identifier)?;
        // A variable passed to many calls is looked up many times, so remember each answer.
        if let Some(&known) = self.bindings.borrow().get(&symbol) {
            return known;
        }
        let written = self.written_once(symbol);
        self.bindings.borrow_mut().insert(symbol, written);
        written
    }

    /// The keys of the object literal that `identifier` is bound to.
    pub fn bound_keys(
        &self,
        identifier: &IdentifierReference<'a>,
    ) -> Option<Rc<ObjectKeys<'a, 'a>>> {
        let symbol = self.symbol(identifier)?;
        // A table of URLs can be read for thousands of keys, so read its keys once.
        if let Some(known) = self.bound_keys.borrow().get(&symbol) {
            return known.clone();
        }
        let keys = match self.bound_expression(identifier) {
            Some(Expression::ObjectExpression(object)) => Some(Rc::new(self.keys(object))),
            _ => None,
        };
        self.bound_keys.borrow_mut().insert(symbol, keys.clone());
        keys
    }

    /// Whether `identifier` names a global, not a variable the script declares.
    pub fn is_global(&self, identifier: &IdentifierReference<'a>) -> bool {
        identifier.reference_id.get().is_some_and(|reference_id| {
            let reference = self.semantic.scoping().get_reference(reference_id);
            reference.symbol_id().is_none()
        })
    }

    fn value_at_depth(&self, expression: &Expression<'a>, depth: u32) -> Option<Value> {
        if !self.take_step(depth) {
            return None;
        }
        let deeper = depth + 1;
        match expression {
            Expression::NumericLiteral(literal) => Some(Value::Number(literal.value)),
            Expression::StringLiteral(literal) => Some(Value::Text(literal.value.as_str()?.into())),
            Expression::TemplateLiteral(template) => {
                Some(Value::Text(template.single_quasi()?.as_str()?.into()))
            }
            Expression::BooleanLiteral(literal) => Some(Value::Bool(literal.value)),
            Expression::NullLiteral(_) => Some(Value::Null),
            Expression::ParenthesizedExpression(inner) => {
                self.value_at_depth(&inner.expression, deeper)
            }
            Expression::Identifier(identifier) => self.identifier_value(identifier, deeper),
            Expression::ArrayExpression(array) => self.array_value(array, deeper),
            Expression::UnaryExpression(unary) => self.unary_value(unary, deeper),
            Expression::LogicalExpression(logical) => self.logical_value(logical, deeper),
            Expression::BinaryExpression(binary) => self.comparison_value(binary, deeper),
            Expression::ConditionalExpression(conditional) => {
                self.conditional_value(conditional, deeper)
            }
            Expression::StaticMemberExpression(member) => self.pixel_ratio_member(member),
            Expression::CallExpression(call) => match self.math_value(call, deeper) {
                Some(value) => Some(value),
                None => self.call_value(call, deeper),
            },
            _ => None,
        }
    }

    fn range_at_depth(&self, expression: &Expression<'a>, depth: u32) -> Option<NumberRange> {
        if !self.take_step(depth) {
            return None;
        }
        let deeper = depth + 1;
        let structural = match expression {
            Expression::ParenthesizedExpression(inner) => {
                return self.range_at_depth(&inner.expression, deeper);
            }
            Expression::Identifier(identifier) => self.identifier_range(identifier, deeper),
            Expression::ConditionalExpression(conditional) => {
                self.conditional_range(conditional, deeper)
            }
            Expression::LogicalExpression(logical) => self.logical_range(logical, deeper),
            Expression::CallExpression(call) => self.call_range(call, deeper),
            _ => None,
        };
        if structural.is_some() {
            return structural;
        }
        let number = self.value_at_depth(expression, deeper)?.as_number()?;
        NumberRange::exactly(number)
    }

    /// Counts one step at `depth`. False once the evaluator is too deep or has taken too many
    /// steps.
    fn take_step(&self, depth: u32) -> bool {
        let steps = self.steps.get().saturating_add(1);
        self.steps.set(steps);
        depth <= MAX_DEPTH && steps <= MAX_STEPS
    }

    fn identifier_value(&self, identifier: &IdentifierReference<'a>, depth: u32) -> Option<Value> {
        if let Some(argument) = self.argument(identifier) {
            return argument.value;
        }
        if !self.is_global(identifier) {
            return self.value_at_depth(self.bound_expression(identifier)?, depth);
        }
        match identifier.name.as_str() {
            DEVICE_PIXEL_RATIO => Some(Value::Number(self.device_scale_factor)),
            GLOBAL_UNDEFINED => Some(Value::Undefined),
            GLOBAL_NAN => Some(Value::Number(f64::NAN)),
            GLOBAL_INFINITY => Some(Value::Number(f64::INFINITY)),
            _ => None,
        }
    }

    fn array_value(&self, array: &ArrayExpression<'a>, depth: u32) -> Option<Value> {
        let elements = array
            .elements
            .iter()
            .map(|element| self.value_at_depth(element.as_expression()?, depth))
            .collect::<Option<Vec<Value>>>()?;
        Some(Value::Array(elements))
    }

    fn unary_value(&self, unary: &UnaryExpression<'a>, depth: u32) -> Option<Value> {
        // `void` gives undefined whatever its operand is.
        if unary.operator == UnaryOperator::Void {
            return Some(Value::Undefined);
        }
        let operand = self.value_at_depth(&unary.argument, depth)?;
        match unary.operator {
            // Minifiers write `true` as `!0` and `false` as `!1`.
            UnaryOperator::LogicalNot => Some(Value::Bool(!operand.is_truthy())),
            UnaryOperator::UnaryNegation => Some(Value::Number(-operand.as_number()?)),
            UnaryOperator::UnaryPlus => Some(Value::Number(operand.as_number()?)),
            _ => None,
        }
    }

    /// `a || b`, `a && b` and `a ?? b`. The left side must be known, or the result could be
    /// either side.
    fn logical_value(&self, logical: &LogicalExpression<'a>, depth: u32) -> Option<Value> {
        let left = self.value_at_depth(&logical.left, depth)?;
        let takes_left = match logical.operator {
            LogicalOperator::Or => left.is_truthy(),
            LogicalOperator::And => !left.is_truthy(),
            LogicalOperator::Coalesce => !matches!(left, Value::Null | Value::Undefined),
        };
        if takes_left {
            Some(left)
        } else {
            self.value_at_depth(&logical.right, depth)
        }
    }

    /// A comparison of two known numbers.
    fn comparison_value(&self, binary: &BinaryExpression<'a>, depth: u32) -> Option<Value> {
        // Check first, so arithmetic spends no steps on operands it cannot use.
        if !binary.operator.is_compare() && !binary.operator.is_equality() {
            return None;
        }
        let left = self.value_at_depth(&binary.left, depth)?.as_number()?;
        let right = self.value_at_depth(&binary.right, depth)?.as_number()?;
        let result = match binary.operator {
            BinaryOperator::LessThan => left < right,
            BinaryOperator::LessEqualThan => left <= right,
            BinaryOperator::GreaterThan => left > right,
            BinaryOperator::GreaterEqualThan => left >= right,
            BinaryOperator::Equality | BinaryOperator::StrictEquality => left == right,
            BinaryOperator::Inequality | BinaryOperator::StrictInequality => left != right,
            _ => return None,
        };
        Some(Value::Bool(result))
    }

    /// `test ? x : y` when `test` is known.
    fn conditional_value(
        &self,
        conditional: &ConditionalExpression<'a>,
        depth: u32,
    ) -> Option<Value> {
        let test = self.value_at_depth(&conditional.test, depth)?;
        let branch = if test.is_truthy() {
            &conditional.consequent
        } else {
            &conditional.alternate
        };
        self.value_at_depth(branch, depth)
    }

    /// `window.devicePixelRatio`, `self.devicePixelRatio` or `globalThis.devicePixelRatio`.
    fn pixel_ratio_member(&self, member: &StaticMemberExpression<'a>) -> Option<Value> {
        let Expression::Identifier(owner) = &member.object else {
            return None;
        };
        let reads_pixel_ratio = member.property.name.as_str() == DEVICE_PIXEL_RATIO
            && PIXEL_RATIO_OWNERS.contains(&owner.name.as_str())
            && self.is_global(owner);
        reads_pixel_ratio.then_some(Value::Number(self.device_scale_factor))
    }

    /// `Math.min(...)` or `Math.max(...)` over known numbers.
    fn math_value(&self, call: &CallExpression<'a>, depth: u32) -> Option<Value> {
        // With no arguments, JavaScript gives the start value: Infinity for min.
        let (start, pick): (f64, fn(f64, f64) -> f64) = match self.math_method(call)? {
            MathMethod::Min => (f64::INFINITY, f64::min),
            MathMethod::Max => (f64::NEG_INFINITY, f64::max),
        };
        let mut result = start;
        for argument in &call.arguments {
            let number = self
                .value_at_depth(argument.as_expression()?, depth)?
                .as_number()?;
            // Like JavaScript, and unlike f64::min, one NaN makes the result NaN.
            if number.is_nan() {
                return Some(Value::Number(f64::NAN));
            }
            result = pick(result, number);
        }
        Some(Value::Number(result))
    }

    /// A parameter of a function being called now: what its argument gave.
    fn argument(&self, identifier: &IdentifierReference<'a>) -> Option<Argument> {
        let symbol = self.symbol(identifier)?;
        self.arguments.borrow().get(&symbol).cloned()
    }

    /// An argument's range, a bound variable's range, or a global number.
    fn identifier_range(
        &self,
        identifier: &IdentifierReference<'a>,
        depth: u32,
    ) -> Option<NumberRange> {
        if let Some(argument) = self.argument(identifier) {
            return argument.range;
        }
        if self.is_global(identifier) {
            return None;
        }
        self.range_at_depth(self.bound_expression(identifier)?, depth)
    }

    /// `test ? x : y`: the branch the test picks, or either branch when the test is not static.
    fn conditional_range(
        &self,
        conditional: &ConditionalExpression<'a>,
        depth: u32,
    ) -> Option<NumberRange> {
        if let Some(test) = self.value_at_depth(&conditional.test, depth) {
            let branch = if test.is_truthy() {
                &conditional.consequent
            } else {
                &conditional.alternate
            };
            return self.range_at_depth(branch, depth);
        }
        let consequent = self.range_at_depth(&conditional.consequent, depth)?;
        let alternate = self.range_at_depth(&conditional.alternate, depth)?;
        Some(consequent.union(alternate))
    }

    /// `a || b` and `a ?? b` where `a` is a number in a known range. `a ?? b` is then `a`; `a ||
    /// b` is `a` when the range has no zero, else either side.
    fn logical_range(&self, logical: &LogicalExpression<'a>, depth: u32) -> Option<NumberRange> {
        if let Some(value) = self.value_at_depth(&logical.left, depth) {
            let takes_left = match logical.operator {
                LogicalOperator::Or => value.is_truthy(),
                LogicalOperator::And => !value.is_truthy(),
                LogicalOperator::Coalesce => !matches!(value, Value::Null | Value::Undefined),
            };
            let side = if takes_left {
                &logical.left
            } else {
                &logical.right
            };
            return self.range_at_depth(side, depth);
        }
        let left = self.range_at_depth(&logical.left, depth)?;
        match logical.operator {
            LogicalOperator::Coalesce => Some(left),
            LogicalOperator::Or if !left.may_be_zero() && !left.may_be_nan => Some(left),
            // A falsy left side, 0 or NaN, gives the right side, so NaN comes only from it.
            LogicalOperator::Or => {
                let right = self.range_at_depth(&logical.right, depth)?;
                Some(NumberRange {
                    may_be_nan: right.may_be_nan,
                    ..left.union(right)
                })
            }
            LogicalOperator::And => None,
        }
    }

    /// `Math.min(...)` and `Math.max(...)` over ranges, or a call to a function with one
    /// `return`. An argument with no range counts as any number.
    fn call_range(&self, call: &CallExpression<'a>, depth: u32) -> Option<NumberRange> {
        match self.math_method(call) {
            Some(MathMethod::Min) => {
                let ranges = self.argument_ranges(call, depth)?;
                Some(NumberRange::min_of(&ranges))
            }
            Some(MathMethod::Max) => {
                let ranges = self.argument_ranges(call, depth)?;
                Some(NumberRange::max_of(&ranges))
            }
            None => {
                self.with_arguments(call, depth, |returns| self.range_at_depth(returns, depth))?
            }
        }
    }

    /// Each argument's range, or an unbounded range when it has none. None for a spread, and for
    /// no arguments, whose value the plain evaluator gives.
    fn argument_ranges(&self, call: &CallExpression<'a>, depth: u32) -> Option<Vec<NumberRange>> {
        if call.arguments.is_empty() {
            return None;
        }
        call.arguments
            .iter()
            .map(|argument| {
                let expression = argument.as_expression()?;
                Some(
                    self.range_at_depth(expression, depth)
                        .unwrap_or(NumberRange::ANY),
                )
            })
            .collect()
    }

    /// The value a call to a function with one `return` gives.
    fn call_value(&self, call: &CallExpression<'a>, depth: u32) -> Option<Value> {
        self.with_arguments(call, depth, |returns| self.value_at_depth(returns, depth))?
    }

    /// Reads the returned expression of the called function with its parameters bound to what
    /// the arguments give. None when the callee is not such a function, or calls itself.
    fn with_arguments<T>(
        &self,
        call: &CallExpression<'a>,
        depth: u32,
        read: impl FnOnce(&'a Expression<'a>) -> T,
    ) -> Option<T> {
        let Expression::Identifier(callee) = call.callee.without_parentheses() else {
            return None;
        };
        let function = self.symbol(callee)?;
        let one_return = self.one_return(function)?;
        // Each argument is read here, where the call is, before the parameters change.
        let mut given = Vec::with_capacity(one_return.parameters.len());
        for (index, &parameter) in one_return.parameters.iter().enumerate() {
            let argument = match call.arguments.get(index) {
                None => Argument {
                    value: Some(Value::Undefined),
                    range: None,
                },
                Some(argument) => {
                    let expression = argument.as_expression()?;
                    Argument {
                        value: self.value_at_depth(expression, depth),
                        range: self.range_at_depth(expression, depth),
                    }
                }
            };
            given.push((parameter, argument));
        }
        if !self.calling.borrow_mut().insert(function) {
            return None;
        }
        let earlier: Vec<(SymbolId, Option<Argument>)> = given
            .into_iter()
            .map(|(parameter, argument)| {
                let earlier = self.arguments.borrow_mut().insert(parameter, argument);
                (parameter, earlier)
            })
            .collect();
        let result = read(one_return.returns);
        for (parameter, earlier) in earlier {
            let mut arguments = self.arguments.borrow_mut();
            match earlier {
                Some(argument) => arguments.insert(parameter, argument),
                None => arguments.remove(&parameter),
            };
        }
        self.calling.borrow_mut().remove(&function);
        Some(result)
    }

    /// The function `symbol` names when its body is one `return` and nothing writes the name
    /// again: a function declaration, or a variable bound once to an arrow or function.
    ///
    /// A function whose parameters repeat a name, or whose body or arguments write a parameter,
    /// is not followed: the evaluator binds each parameter once, to its argument's value.
    fn one_return(&self, symbol: SymbolId) -> Option<OneReturn<'a>> {
        let one_return = self.declared_one_return(symbol)?;
        let scoping = self.semantic.scoping();
        let mut distinct = one_return.parameters.clone();
        distinct.sort_unstable();
        distinct.dedup();
        let has_a_written_parameter = one_return.parameters.iter().any(|&parameter| {
            scoping
                .get_resolved_references(parameter)
                .any(|reference| reference.is_write())
        });
        let is_plain = distinct.len() == one_return.parameters.len() && !has_a_written_parameter;
        is_plain.then_some(one_return)
    }

    fn declared_one_return(&self, symbol: SymbolId) -> Option<OneReturn<'a>> {
        let scoping = self.semantic.scoping();
        if !scoping.symbol_redeclarations(symbol).is_empty() {
            return None;
        }
        match self.semantic.symbol_declaration(symbol).kind() {
            AstKind::Function(function) => {
                let is_written = scoping
                    .get_resolved_references(symbol)
                    .any(|reference| reference.is_write());
                if is_written || function.generator || function.r#async {
                    return None;
                }
                one_return_of(&function.params, function.body.as_ref()?)
            }
            AstKind::VariableDeclarator(_) => match self.written_once(symbol)? {
                Expression::ArrowFunctionExpression(arrow) if !arrow.r#async => {
                    let returns = match &arrow.body {
                        ArrowFunctionBody::FunctionBody(body) => {
                            return one_return_of(&arrow.params, body);
                        }
                        body => body.as_expression()?,
                    };
                    Some(OneReturn {
                        parameters: parameter_symbols(&arrow.params)?,
                        returns,
                    })
                }
                Expression::FunctionExpression(function)
                    if !function.generator && !function.r#async =>
                {
                    one_return_of(&function.params, function.body.as_ref()?)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Which of `Math.min` and `Math.max` the call is, when `Math` is the global.
    fn math_method(&self, call: &CallExpression<'a>) -> Option<MathMethod> {
        let Expression::StaticMemberExpression(callee) = &call.callee else {
            return None;
        };
        let Expression::Identifier(math) = &callee.object else {
            return None;
        };
        if math.name.as_str() != MATH || !self.is_global(math) {
            return None;
        }
        match callee.property.name.as_str() {
            MATH_MIN => Some(MathMethod::Min),
            MATH_MAX => Some(MathMethod::Max),
            _ => None,
        }
    }

    /// The one expression written to `symbol`, as `bound_expression` describes.
    /// The variable that `identifier` names. None for a global.
    fn symbol(&self, identifier: &IdentifierReference<'a>) -> Option<SymbolId> {
        let reference_id = identifier.reference_id.get()?;
        self.semantic
            .scoping()
            .get_reference(reference_id)
            .symbol_id()
    }

    fn written_once(&self, symbol: SymbolId) -> Option<&'a Expression<'a>> {
        let scoping = self.semantic.scoping();
        if !scoping.symbol_redeclarations(symbol).is_empty() {
            return None;
        }
        let AstKind::VariableDeclarator(declarator) =
            self.semantic.symbol_declaration(symbol).kind()
        else {
            return None;
        };
        if !matches!(declarator.id, BindingPattern::BindingIdentifier(_)) {
            return None;
        }
        let mut writes: Vec<&'a Expression<'a>> = declarator.init.iter().collect();
        for reference in scoping.get_resolved_references(symbol) {
            if !reference.is_write() {
                continue;
            }
            match self.semantic.nodes().parent_kind(reference.node_id()) {
                AstKind::AssignmentExpression(assignment)
                    if assignment.operator == AssignmentOperator::Assign =>
                {
                    writes.push(&assignment.right);
                }
                // `x += 1`, `x++`, destructuring and loop heads write values we do not follow.
                _ => return None,
            }
        }
        match writes.as_slice() {
            [Expression::ObjectExpression(_)] if self.has_key_writes(symbol) => None,
            [only] => Some(only),
            _ => None,
        }
    }

    /// Whether the script writes a key of the value of `symbol`: `o.x = 1`, `o.x.y += 1`,
    /// `o.x++`, `delete o.x`, a destructuring target such as `[o.x] = a`, or a call in
    /// `KEY_WRITING_CALLS` with `o` or `o.x` first.
    fn has_key_writes(&self, symbol: SymbolId) -> bool {
        let nodes = self.semantic.nodes();
        self.semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|reference| {
                let mut node = reference.node_id();
                let mut is_member = false;
                // Climb `o.x.y`: each member expression whose object is the node below it.
                loop {
                    let parent = nodes.parent_id(node);
                    let object = match nodes.kind(parent) {
                        AstKind::StaticMemberExpression(member) => member.object.span(),
                        AstKind::ComputedMemberExpression(member) => member.object.span(),
                        _ => break,
                    };
                    if object != nodes.kind(node).span() {
                        break;
                    }
                    node = parent;
                    is_member = true;
                }
                let span = nodes.kind(node).span();
                match nodes.parent_kind(node) {
                    AstKind::CallExpression(call) => writes_keys_of_first(call, span),
                    _ if !is_member => false,
                    AstKind::AssignmentExpression(assignment) => assignment.left.span() == span,
                    AstKind::AssignmentTargetWithDefault(target) => target.binding.span() == span,
                    AstKind::ForInStatement(each) => each.left.span() == span,
                    AstKind::ForOfStatement(each) => each.left.span() == span,
                    AstKind::UnaryExpression(unary) => unary.operator == UnaryOperator::Delete,
                    AstKind::UpdateExpression(_)
                    | AstKind::ArrayAssignmentTarget(_)
                    | AstKind::AssignmentTargetPropertyProperty(_) => true,
                    _ => false,
                }
            })
    }

    fn add_keys<'e>(
        &self,
        object: &'e ObjectExpression<'a>,
        keys: &mut ObjectKeys<'e, 'a>,
        depth: u32,
    ) {
        for property in &object.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    let name = property.key.static_name();
                    match name.as_ref().and_then(|name| name.as_str()) {
                        Some(name) => keys.write(name, &property.value),
                        // The key may replace any key before it, or add any other.
                        None => {
                            keys.hide_every_value();
                            keys.has_computed_key = true;
                        }
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    let deeper = depth + 1;
                    let inner = if self.take_step(deeper) {
                        self.object(&spread.argument)
                    } else {
                        None
                    };
                    match inner {
                        Some(inner) => self.add_keys(inner, keys, deeper),
                        // The spread may replace any key before it, or add any other.
                        None => {
                            keys.hide_every_value();
                            keys.has_unread_spread = true;
                        }
                    }
                }
            }
        }
    }
}

/// `Math.min` or `Math.max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MathMethod {
    Min,
    Max,
}

impl NumberRange {
    /// What a value with no range may give `Math.min` or `Math.max`: any number, or NaN.
    pub const ANY: NumberRange = NumberRange {
        low: None,
        high: None,
        may_be_nan: true,
    };

    /// A range of one number. None for NaN, which the evaluator never bounds.
    pub fn exactly(number: f64) -> Option<NumberRange> {
        (!number.is_nan()).then_some(NumberRange {
            low: Some(number),
            high: Some(number),
            may_be_nan: false,
        })
    }

    /// The numbers either range holds.
    fn union(self, other: NumberRange) -> NumberRange {
        NumberRange {
            low: both(self.low, other.low, f64::min),
            high: both(self.high, other.high, f64::max),
            may_be_nan: self.may_be_nan || other.may_be_nan,
        }
    }

    /// Whether 0 is in the range, so `||` may take its right side.
    fn may_be_zero(self) -> bool {
        self.low.is_none_or(|low| low <= 0.0) && self.high.is_none_or(|high| high >= 0.0)
    }

    /// The range of `Math.min` over numbers in these ranges.
    fn min_of(ranges: &[NumberRange]) -> NumberRange {
        NumberRange {
            low: ranges
                .iter()
                .try_fold(f64::INFINITY, |lowest, range| Some(lowest.min(range.low?))),
            high: ranges
                .iter()
                .filter_map(|range| range.high)
                .reduce(f64::min),
            may_be_nan: ranges.iter().any(|range| range.may_be_nan),
        }
    }

    /// The range of `Math.max` over numbers in these ranges.
    fn max_of(ranges: &[NumberRange]) -> NumberRange {
        NumberRange {
            low: ranges.iter().filter_map(|range| range.low).reduce(f64::max),
            high: ranges.iter().try_fold(f64::NEG_INFINITY, |highest, range| {
                Some(highest.max(range.high?))
            }),
            may_be_nan: ranges.iter().any(|range| range.may_be_nan),
        }
    }
}

/// `pick` of two bounds, or None when either side is unbounded.
fn both(a: Option<f64>, b: Option<f64>, pick: fn(f64, f64) -> f64) -> Option<f64> {
    Some(pick(a?, b?))
}

/// A function body that is one `return` of an expression, with plain parameters.
fn one_return_of<'a>(
    parameters: &FormalParameters<'a>,
    body: &'a FunctionBody<'a>,
) -> Option<OneReturn<'a>> {
    if !body.directives.is_empty() {
        return None;
    }
    let [Statement::ReturnStatement(only)] = body.statements.as_slice() else {
        return None;
    };
    Some(OneReturn {
        parameters: parameter_symbols(parameters)?,
        returns: only.argument.as_ref()?,
    })
}

/// Each parameter's symbol. None for a rest parameter, a default value or a destructured
/// parameter, whose values the evaluator does not follow.
fn parameter_symbols(parameters: &FormalParameters<'_>) -> Option<Vec<SymbolId>> {
    if parameters.rest.is_some() {
        return None;
    }
    parameters
        .items
        .iter()
        .map(|parameter| {
            if parameter.initializer.is_some() {
                return None;
            }
            match &parameter.pattern {
                BindingPattern::BindingIdentifier(identifier) => identifier.symbol_id.get(),
                _ => None,
            }
        })
        .collect()
}

/// Whether `call` is in `KEY_WRITING_CALLS` and its first argument is at `argument`.
fn writes_keys_of_first(call: &CallExpression<'_>, argument: Span) -> bool {
    let is_first = call
        .arguments
        .first()
        .is_some_and(|first| first.span() == argument);
    let name = call
        .callee
        .as_member_expression()
        .and_then(|member| member.static_property_name());
    is_first
        && name.is_some_and(|name| KEY_WRITING_CALLS.iter().any(|call_name| name == *call_name))
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_semantic::SemanticBuilder;

    use super::*;
    use crate::bundle::js;

    const DEVICE_SCALE_FACTOR: f64 = 3.0;

    /// What an object literal says about one key, with the key's value read.
    #[derive(Debug, PartialEq)]
    enum Found {
        Absent,
        Hidden,
        Written(Option<Value>),
    }

    /// Runs `read` on the expression of the last statement in `source`.
    fn with_last_expression<T>(
        source: &str,
        read: impl for<'a> FnOnce(&Evaluator<'_, 'a>, &Expression<'a>) -> T,
    ) -> T {
        let allocator = Allocator::default();
        let program = js::parse(&allocator, source).expect(source);
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(program)
            .semantic;
        let Some(Statement::ExpressionStatement(last)) = program.body.last() else {
            panic!("{source} does not end in an expression");
        };
        read(
            &Evaluator::new(&semantic, DEVICE_SCALE_FACTOR),
            &last.expression,
        )
    }

    fn evaluate(source: &str) -> Option<Value> {
        with_last_expression(source, |evaluator, expression| evaluator.value(expression))
    }

    /// What the object literal in the last statement of `source` says about `key`.
    fn property(source: &str, key: &str) -> Found {
        with_last_expression(source, |evaluator, expression| {
            let object = evaluator.object(expression).expect("an object literal");
            match evaluator.keys(object).get(key) {
                Property::Absent => Found::Absent,
                Property::Hidden => Found::Hidden,
                Property::Written(value) => Found::Written(evaluator.value(value)),
            }
        })
    }

    /// Whether the last statement of `source` is an object literal or a binding followed to one.
    fn is_followed_to_an_object(source: &str) -> bool {
        with_last_expression(source, |evaluator, expression| {
            evaluator.object(expression).is_some()
        })
    }

    fn number(value: f64) -> Option<Value> {
        Some(Value::Number(value))
    }

    fn text(value: &str) -> Option<Value> {
        Some(Value::Text(value.into()))
    }

    /// `const a0 = 1, a1 = a0, ...; a<links>`: one binding per link.
    fn chain_of_bindings(links: u32) -> String {
        let bindings: Vec<String> = (1..=links)
            .map(|link| format!("a{link} = a{}", link - 1))
            .collect();
        format!("const a0 = 1, {}; a{links}", bindings.join(", "))
    }

    /// `const a = [1, 1, ...], b = [a, a, ...]; b`, with `width` elements in each array.
    fn array_of_arrays(width: usize) -> String {
        let ones = vec!["1"; width].join(", ");
        let rows = vec!["a"; width].join(", ");
        format!("const a = [{ones}], b = [{rows}]; b")
    }

    #[test]
    fn literals_are_their_own_values() {
        assert_eq!(evaluate("1.5"), number(1.5));
        assert_eq!(evaluate("3e3"), number(3000.0));
        // In parentheses, so the string is not a directive.
        assert_eq!(evaluate("('webgl')"), text("webgl"));
        assert_eq!(evaluate("`webgpu`"), text("webgpu"));
        assert_eq!(evaluate("null"), Some(Value::Null));
    }

    #[test]
    fn global_constants_are_their_values() {
        assert_eq!(evaluate("undefined"), Some(Value::Undefined));
        assert_eq!(evaluate("Infinity"), number(f64::INFINITY));
        assert!(evaluate("NaN").unwrap().as_number().unwrap().is_nan());
    }

    #[test]
    fn unary_minus_and_plus_take_a_known_number() {
        assert_eq!(evaluate("-2"), number(-2.0));
        assert_eq!(evaluate("+2"), number(2.0));
        assert_eq!(evaluate("-x"), None);
    }

    #[test]
    fn void_is_undefined_whatever_its_operand() {
        assert_eq!(evaluate("void 0"), Some(Value::Undefined));
        assert_eq!(evaluate("void f()"), Some(Value::Undefined));
    }

    #[test]
    fn minified_booleans_are_booleans() {
        assert_eq!(evaluate("!0"), Some(Value::Bool(true)));
        assert_eq!(evaluate("!1"), Some(Value::Bool(false)));
    }

    #[test]
    fn an_array_is_known_when_every_element_is() {
        assert_eq!(
            evaluate("[0, 500, !0]"),
            Some(Value::Array(vec![
                Value::Number(0.0),
                Value::Number(500.0),
                Value::Bool(true)
            ]))
        );
        assert_eq!(evaluate("[0, x]"), None);
        assert_eq!(evaluate("[0, ...x]"), None);
    }

    #[test]
    fn device_pixel_ratio_reads_the_profile_through_any_global_owner() {
        for source in [
            "devicePixelRatio",
            "window.devicePixelRatio",
            "self.devicePixelRatio",
            "globalThis.devicePixelRatio",
        ] {
            assert_eq!(evaluate(source), number(DEVICE_SCALE_FACTOR), "{source}");
        }
    }

    #[test]
    fn a_local_binding_shadows_the_global() {
        assert_eq!(evaluate("let window = {}; window.devicePixelRatio"), None);
        assert_eq!(
            evaluate("let devicePixelRatio = q(); devicePixelRatio"),
            None
        );
        assert_eq!(evaluate("let Math = q(); Math.min(1, 2)"), None);
    }

    #[test]
    fn a_parameter_does_not_shadow_the_global_outside_its_function() {
        assert_eq!(
            evaluate("function f(devicePixelRatio) {} devicePixelRatio"),
            number(DEVICE_SCALE_FACTOR)
        );
    }

    #[test]
    fn math_min_and_max_take_known_numbers() {
        assert_eq!(evaluate("Math.min(devicePixelRatio, 2)"), number(2.0));
        assert_eq!(
            evaluate("Math.max(1, window.devicePixelRatio)"),
            number(3.0)
        );
        assert_eq!(evaluate("Math.min(devicePixelRatio, x)"), None);
    }

    #[test]
    fn math_min_and_max_follow_javascript_at_the_edges() {
        assert_eq!(evaluate("Math.min()"), number(f64::INFINITY));
        assert!(
            evaluate("Math.max(1, NaN)")
                .unwrap()
                .as_number()
                .unwrap()
                .is_nan()
        );
    }

    #[test]
    fn another_math_method_is_not_static() {
        assert_eq!(evaluate("Math.abs(-1)"), None);
    }

    #[test]
    fn or_takes_the_left_side_only_when_it_is_known_and_truthy() {
        assert_eq!(evaluate("window.devicePixelRatio || 1"), number(3.0));
        assert_eq!(evaluate("0 || 1"), number(1.0));
        assert_eq!(evaluate("x.resolution || 1"), None);
    }

    #[test]
    fn coalesce_takes_the_right_side_only_when_the_left_is_known_nullish() {
        assert_eq!(evaluate("void 0 ?? 2"), number(2.0));
        assert_eq!(evaluate("0 ?? 2"), number(0.0));
        assert_eq!(evaluate("x.resolution ?? 2"), None);
    }

    #[test]
    fn and_takes_the_right_side_only_when_the_left_is_known_and_truthy() {
        assert_eq!(evaluate("1 && 2"), number(2.0));
        assert_eq!(evaluate("0 && 2"), number(0.0));
        assert_eq!(evaluate("x && 2"), None);
    }

    #[test]
    fn a_comparison_of_known_numbers_is_a_boolean() {
        assert_eq!(evaluate("devicePixelRatio === 3"), Some(Value::Bool(true)));
        assert_eq!(evaluate("devicePixelRatio != 3"), Some(Value::Bool(false)));
    }

    #[test]
    fn arithmetic_is_not_static() {
        assert_eq!(evaluate("1 * 2"), None);
    }

    #[test]
    fn a_conditional_needs_a_known_test() {
        assert_eq!(evaluate("devicePixelRatio > 2 ? 2 : 1"), number(2.0));
        assert_eq!(evaluate("devicePixelRatio <= 2 ? 2 : 1"), number(1.0));
        assert_eq!(evaluate("x > 2 ? 2 : 1"), None);
    }

    #[test]
    fn a_constant_is_followed_to_its_one_value() {
        assert_eq!(
            evaluate("const r = Math.min(window.devicePixelRatio, 2); r"),
            number(2.0)
        );
        assert_eq!(evaluate("const a = 2, b = a; b"), number(2.0));
    }

    #[test]
    fn a_variable_assigned_once_after_its_declaration_is_followed() {
        assert_eq!(
            evaluate("var v; function init() { v = '8.22.0'; } v"),
            text("8.22.0")
        );
    }

    #[test]
    fn a_variable_written_twice_is_not_static() {
        assert_eq!(evaluate("let r = 1; r = 2; r"), None);
        assert_eq!(evaluate("var r; r = 1; r = 2; r"), None);
    }

    #[test]
    fn a_compound_assignment_or_update_is_not_followed() {
        assert_eq!(evaluate("let r = 1; r += 1; r"), None);
        assert_eq!(evaluate("let r = 1; r++; r"), None);
        assert_eq!(evaluate("var r; r += 2; r"), None);
    }

    #[test]
    fn a_redeclared_variable_is_not_static() {
        assert_eq!(evaluate("var r = 1; var r = 2; r"), None);
    }

    #[test]
    fn a_variable_with_no_write_is_not_static() {
        assert_eq!(evaluate("var r; r"), None);
    }

    #[test]
    fn destructured_and_parameter_bindings_are_not_static() {
        assert_eq!(evaluate("const [r] = [2]; r"), None);
        assert_eq!(evaluate("const { r } = { r: 2 }; r"), None);
        assert_eq!(evaluate("let r = 1; [r] = [2]; r"), None);
    }

    #[test]
    fn circular_bindings_give_up_instead_of_looping() {
        assert_eq!(evaluate("var a, b; a = b; b = a; a"), None);
    }

    #[test]
    fn a_chain_of_bindings_deeper_than_the_limit_is_not_static() {
        // A chain of n bindings is n + 1 expressions deep.
        assert_eq!(evaluate(&chain_of_bindings(MAX_DEPTH - 1)), number(1.0));
        assert_eq!(evaluate(&chain_of_bindings(MAX_DEPTH)), None);
    }

    #[test]
    fn a_value_that_takes_too_many_steps_is_not_static() {
        assert!(evaluate(&array_of_arrays(20)).is_some());
        assert_eq!(evaluate(&array_of_arrays(40)), None);
    }

    #[test]
    fn a_spread_of_an_object_literal_adds_its_keys() {
        assert_eq!(
            property("({ ...{ a: 1 } })", "a"),
            Found::Written(number(1.0))
        );
        assert_eq!(property("({ ...{ a: 1 } })", "b"), Found::Absent);
    }

    #[test]
    fn a_key_after_a_spread_wins() {
        assert_eq!(
            property("const d = { a: 'x' }; ({ ...d, a: 1 })", "a"),
            Found::Written(number(1.0))
        );
    }

    #[test]
    fn a_spread_after_a_key_wins() {
        assert_eq!(
            property("({ a: 1, ...{ a: 'x' } })", "a"),
            Found::Written(text("x"))
        );
    }

    #[test]
    fn a_spread_that_is_not_an_object_literal_hides_every_key_before_it() {
        assert_eq!(property("({ a: 1, ...q })", "a"), Found::Hidden);
        assert_eq!(property("({ a: 1, ...q })", "b"), Found::Hidden);
        assert_eq!(
            property("({ ...q, a: 1 })", "a"),
            Found::Written(number(1.0))
        );
    }

    #[test]
    fn a_computed_key_hides_every_key_before_it() {
        assert_eq!(property("({ a: 1, [k]: 2 })", "a"), Found::Hidden);
        assert_eq!(property("({ a: 1, [k]: 2 })", "b"), Found::Hidden);
        assert_eq!(
            property("({ [k]: 2, a: 1 })", "a"),
            Found::Written(number(1.0))
        );
    }

    #[test]
    fn a_key_written_after_a_computed_key_is_known() {
        assert_eq!(
            property("({ a: 1, [k]: 2, b: 3 })", "b"),
            Found::Written(number(3.0))
        );
    }

    #[test]
    fn a_binding_read_twice_gives_the_same_value() {
        assert_eq!(
            evaluate("const r = 2, o = [r, r]; o"),
            Some(Value::Array(vec![Value::Number(2.0), Value::Number(2.0)]))
        );
    }

    #[test]
    fn a_binding_to_an_object_whose_keys_are_only_read_is_followed() {
        let reads = [
            "f(o.a, o['a'], o.a.b)",
            "f(o)",
            "g = o.a",
            "typeof o.a",
            "[x = o.a] = q",
            "for (k in o.a);",
            "for (k of o.a);",
            "q[o.a] = 1",
        ];
        for read in reads {
            let source = format!("const o = {{a: 1}}; {read}; o");
            assert!(is_followed_to_an_object(&source), "{read}");
        }
    }

    #[test]
    fn a_binding_to_an_object_whose_keys_are_written_is_not_followed() {
        let writes = [
            "o.a = 2",
            "o['a'] = 2",
            "o.a.b = 2",
            "o.a += 2",
            "o.a++",
            "delete o.a",
            "[o.a] = q",
            "({k: o.a} = q)",
            "[o.a = 1] = q",
            "for (o.a in q);",
            "for (o.a of q);",
        ];
        for write in writes {
            let source = format!("const o = {{a: 1}}; {write}; o");
            assert!(!is_followed_to_an_object(&source), "{write}");
        }
    }

    #[test]
    fn a_binding_passed_first_to_a_key_writing_call_is_not_followed() {
        let calls = [
            "Object.assign(o, q)",
            "Object.assign(o.a, q)",
            "Object.defineProperty(o, 'a', q)",
        ];
        for call in calls {
            let source = format!("const o = {{a: 1}}; {call}; o");
            assert!(!is_followed_to_an_object(&source), "{call}");
        }
    }

    #[test]
    fn a_binding_passed_later_to_a_key_writing_call_is_followed() {
        assert!(is_followed_to_an_object(
            "const o = {a: 1}; Object.assign({}, o); o"
        ));
    }

    #[test]
    fn a_self_spreading_binding_gives_up_instead_of_looping() {
        let source = "var o; o = { ...o, a: 1 }; o";
        assert_eq!(property(source, "a"), Found::Written(number(1.0)));
        assert_eq!(property(source, "b"), Found::Hidden);
    }

    fn range(source: &str) -> Option<NumberRange> {
        with_last_expression(source, |evaluator, expression| {
            evaluator.number_range(expression)
        })
    }

    fn between(low: Option<f64>, high: Option<f64>) -> Option<NumberRange> {
        Some(NumberRange {
            low,
            high,
            may_be_nan: false,
        })
    }

    fn maybe_nan(low: Option<f64>, high: Option<f64>) -> Option<NumberRange> {
        Some(NumberRange {
            low,
            high,
            may_be_nan: true,
        })
    }

    #[test]
    fn a_static_number_is_a_range_of_one_value() {
        assert_eq!(range("devicePixelRatio"), between(Some(3.0), Some(3.0)));
        assert_eq!(range("NaN"), None);
        assert_eq!(range("('2')"), None);
    }

    #[test]
    fn a_conditional_with_an_unknown_test_takes_either_branch() {
        assert_eq!(range("q() ? 1.5 : 2"), between(Some(1.5), Some(2.0)));
        assert_eq!(range("q() ? 1.5 : r.x"), None);
        assert_eq!(
            range("devicePixelRatio > 2 ? 2 : r.x"),
            between(Some(2.0), Some(2.0))
        );
    }

    #[test]
    fn math_min_and_max_bound_an_unknown_argument() {
        assert_eq!(range("Math.min(r.x, 2)"), maybe_nan(None, Some(2.0)));
        assert_eq!(range("Math.max(r.x, 1)"), maybe_nan(Some(1.0), None));
        assert_eq!(
            range("Math.min(devicePixelRatio, q() ? 1.5 : 2)"),
            between(Some(1.5), Some(2.0))
        );
        assert_eq!(
            range("Math.min()"),
            between(Some(f64::INFINITY), Some(f64::INFINITY))
        );
        assert_eq!(range("Math.min(...r)"), None);
    }

    #[test]
    fn or_takes_a_left_range_with_no_zero_and_else_either_side() {
        assert_eq!(range("(q() ? 1 : 2) || 5"), between(Some(1.0), Some(2.0)));
        assert_eq!(range("(q() ? 0 : 2) || 5"), between(Some(0.0), Some(5.0)));
        assert_eq!(range("(q() ? 1 : 2) ?? 5"), between(Some(1.0), Some(2.0)));
        assert_eq!(range("r.x || 1"), None);
    }

    #[test]
    fn a_function_with_one_return_is_followed_with_its_arguments() {
        let declared = "function f(e, t) { return Math.min(e || 1, t ? 1.5 : 2) }";
        assert_eq!(
            range(&format!("{declared} f(devicePixelRatio, q())")),
            between(Some(1.5), Some(2.0))
        );
        assert_eq!(
            evaluate(&format!("{declared} f(devicePixelRatio, !0)")),
            number(1.5)
        );
        assert_eq!(
            evaluate(&format!("{declared} f(devicePixelRatio)")),
            number(2.0)
        );
    }

    #[test]
    fn an_arrow_or_function_bound_once_is_followed() {
        assert_eq!(
            evaluate("const f = (e) => Math.min(e, 2); f(3)"),
            number(2.0)
        );
        assert_eq!(
            evaluate("const f = (e) => { return Math.min(e, 2) }; f(3)"),
            number(2.0)
        );
        assert_eq!(
            evaluate("const f = function (e) { return e }; f(3)"),
            number(3.0)
        );
    }

    #[test]
    fn a_function_that_is_not_one_return_is_not_followed() {
        assert_eq!(evaluate("function f(e) { g(); return e } f(3)"), None);
        assert_eq!(evaluate("function f(e = 1) { return e } f(3)"), None);
        assert_eq!(evaluate("function f(...e) { return 2 } f(3)"), None);
        assert_eq!(evaluate("function f({e}) { return 2 } f(3)"), None);
        assert_eq!(evaluate("async function f() { return 2 } f()"), None);
        assert_eq!(evaluate("function f() { return 2 } f = g; f()"), None);
        assert_eq!(evaluate("let f = () => 2; f = g; f()"), None);
    }

    #[test]
    fn a_function_that_calls_itself_gives_up_instead_of_looping() {
        assert_eq!(evaluate("function f(e) { return f(e) } f(3)"), None);
        assert_eq!(range("function f(e) { return f(e) } f(3)"), None);
    }

    #[test]
    fn a_call_inside_the_body_reads_its_own_arguments() {
        assert_eq!(
            evaluate("function g(x) { return x } function f(x) { return g(2) } f(3)"),
            number(2.0)
        );
        assert_eq!(
            evaluate("function g(x) { return x } function f(x) { return g(x) } f(3)"),
            number(3.0)
        );
    }

    #[test]
    fn a_function_that_writes_a_parameter_is_not_followed() {
        assert_eq!(
            evaluate("function g(a, b) { return b } function f(x) { return g(x = 3, x) } f(1)"),
            None
        );
    }

    #[test]
    fn a_function_with_a_repeated_parameter_is_not_followed() {
        assert_eq!(evaluate("function f(x, x) { return x } f(1, 3)"), None);
    }

    #[test]
    fn a_range_that_may_be_nan_takes_the_right_side_of_or() {
        assert_eq!(
            range("Math.min(Math.max(window.x, 1), 2) || 3"),
            Some(NumberRange {
                low: Some(1.0),
                high: Some(3.0),
                may_be_nan: false,
            })
        );
        assert_eq!(
            range("Math.max('oops', 3) || 1"),
            Some(NumberRange {
                low: Some(1.0),
                high: None,
                may_be_nan: false,
            })
        );
        assert!(range("Math.min(r.x, 2)").unwrap().may_be_nan);
    }

    #[test]
    fn a_parameter_read_outside_a_call_is_not_static() {
        assert_eq!(evaluate("function f(e) { return e } e"), None);
    }
}
