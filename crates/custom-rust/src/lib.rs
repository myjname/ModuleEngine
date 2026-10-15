//! A deliberately small, source-preserving Rust data-flow editor.
//!
//! `syn` parses real Rust. The preview understands scalar `f64` arithmetic;
//! everything else stays in the source and is shown as an opaque node. Edits
//! patch spans in the original source instead of pretty-printing the file.

mod command;
pub use command::*;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use proc_macro2::Span;
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::{BinOp, Expr, FnArg, Item, ItemFn, Lit, Pat, ReturnType, Stmt, Type, UnOp};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CodeNodeKind {
    Input { name: String },
    Number { value: f64 },
    Binary { op: String },
    Call { name: String },
    Output,
    Opaque { source: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeNode {
    pub id: String,
    pub label: String,
    pub kind: CodeNodeKind,
    pub position: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeEdge {
    pub from: String,
    pub to: String,
    pub input: usize,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum CustomError {
    #[error("Rust syntax error: {0}")]
    Parse(String),
    #[error("Unsupported Rust preview: {0}")]
    Unsupported(String),
    #[error("Cannot edit Rust graph: {0}")]
    InvalidEdit(String),
    #[error("Missing f64 input `{0}`")]
    MissingInput(String),
    #[error("Rust preview failed: {0}")]
    Evaluation(String),
}

#[derive(Clone, Debug)]
struct Binding {
    name: String,
    visible_after: usize,
}

#[derive(Clone, Debug)]
struct NodeSource {
    range: Option<Range<usize>>,
    inputs: Vec<Range<usize>>,
    operator: Option<Range<usize>>,
    references: BTreeMap<String, usize>,
    bindings: Vec<Binding>,
    statement_start: usize,
    depth: usize,
    scalar: bool,
}

#[derive(Clone, Debug)]
pub struct CustomProgram {
    source: String,
    function_name: String,
    nodes: Vec<CodeNode>,
    edges: Vec<CodeEdge>,
    locations: BTreeMap<String, NodeSource>,
    unsupported: Vec<String>,
    scalar_signature: bool,
}

impl CustomProgram {
    /// Parse a file containing `speed`, or exactly one free function. Other
    /// file items are retained byte-for-byte and are not executed by preview.
    pub fn parse(source: &str) -> Result<Self, CustomError> {
        let file = syn::parse_file(source).map_err(|err| CustomError::Parse(err.to_string()))?;
        let functions: Vec<_> = file
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Fn(function) = item {
                    Some(function)
                } else {
                    None
                }
            })
            .collect();
        let function = functions
            .iter()
            .copied()
            .find(|function| function.sig.ident == "speed")
            .or_else(|| (functions.len() == 1).then(|| functions[0]))
            .ok_or_else(|| {
                CustomError::Parse("expected `fn speed(...)`, or one free function".into())
            })?;
        let mut builder = Builder::new(source, function.sig.ident.to_string());
        if file
            .attrs
            .iter()
            .any(|attribute| !attribute.path().is_ident("doc"))
        {
            builder.program.scalar_signature = false;
            builder
                .program
                .unsupported
                .push("file attributes other than documentation are not interpreted".into());
        }
        builder.function(function)?;
        Ok(builder.program)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn function_name(&self) -> &str {
        &self.function_name
    }
    pub fn nodes(&self) -> &[CodeNode] {
        &self.nodes
    }
    pub fn edges(&self) -> &[CodeEdge] {
        &self.edges
    }

    /// Whether the limited type analysis establishes a scalar port. Opaque
    /// dependencies and unsupported signatures must be displayed as unknown.
    pub fn node_is_scalar(&self, node_id: &str) -> bool {
        self.scalar_signature
            && self
                .locations
                .get(node_id)
                .is_some_and(|location| location.scalar)
    }

    /// Change a literal without reformatting its surrounding Rust.
    pub fn set_number(&mut self, node_id: &str, value: f64) -> Result<(), CustomError> {
        if !value.is_finite() {
            return Err(CustomError::InvalidEdit(
                "numeric literals must be finite".into(),
            ));
        }
        if !matches!(self.node(node_id)?.kind, CodeNodeKind::Number { .. }) {
            return Err(CustomError::InvalidEdit(
                "the selected node is not a number".into(),
            ));
        }
        let range = self.location(node_id)?.range.clone().ok_or_else(|| {
            CustomError::InvalidEdit("this number has no editable source span".into())
        })?;
        // Preserve inference in retained code; forcing an f64 suffix onto an
        // unsuffixed literal can break an opaque use that expects f32.
        let suffix = if self.source[range.clone()].trim_end().ends_with("f64") {
            "f64"
        } else {
            ""
        };
        let literal = format!("{value:?}{suffix}");
        let replacement = with_comments(&self.source[range.clone()], &literal);
        self.apply(vec![(range, replacement)])
    }

    /// Supported scalar operators are `+`, `-`, `*`, `/`, and `%`.
    pub fn set_binary_op(&mut self, node_id: &str, op: &str) -> Result<(), CustomError> {
        if !["+", "-", "*", "/", "%"].contains(&op) {
            return Err(CustomError::InvalidEdit(format!(
                "`{op}` is not an f64 arithmetic operator"
            )));
        }
        if !matches!(self.node(node_id)?.kind, CodeNodeKind::Binary { .. }) {
            return Err(CustomError::InvalidEdit(
                "the selected node is not a binary operator".into(),
            ));
        }
        if !self.location(node_id)?.scalar {
            return Err(CustomError::InvalidEdit(
                "operator type depends on opaque Rust".into(),
            ));
        }
        let range = self.location(node_id)?.operator.clone().ok_or_else(|| {
            CustomError::InvalidEdit("operator source span is unavailable".into())
        })?;
        self.apply(vec![(range, op.into())])
    }

    /// Replace one scalar input. Unbound expressions are lifted into a local
    /// `let` so both uses share one graph node, rather than copying a subtree.
    /// A mutation is transactional: an error leaves source and graph intact.
    pub fn connect(&mut self, from_id: &str, to_id: &str, input: usize) -> Result<(), CustomError> {
        if !self.scalar_signature {
            return Err(CustomError::InvalidEdit(
                "rewiring requires a plain function with named f64 parameters and f64 return type"
                    .into(),
            ));
        }
        let from = self.node(from_id)?;
        self.node(to_id)?;
        let from_location = self.location(from_id)?;
        let to_location = self.location(to_id)?;
        if !from_location.scalar || !to_location.scalar {
            return Err(CustomError::InvalidEdit(
                "connections require known f64 ports; opaque Rust has unknown type".into(),
            ));
        }
        let target = to_location.inputs.get(input).cloned().ok_or_else(|| {
            CustomError::InvalidEdit(format!("node `{to_id}` has no input {input}"))
        })?;
        if matches!(from.kind, CodeNodeKind::Output) {
            return Err(CustomError::InvalidEdit(
                "the function output has no outgoing port".into(),
            ));
        }
        if self
            .edges
            .iter()
            .any(|edge| edge.from == from_id && edge.to == to_id && edge.input == input)
        {
            return Ok(());
        }
        if from_id == to_id || self.reaches(to_id, from_id) {
            return Err(CustomError::InvalidEdit(
                "this connection would create a cycle".into(),
            ));
        }
        // Replacing an expression must never silently delete an opaque snippet.
        if self.nodes.iter().any(|node| {
            matches!(node.kind, CodeNodeKind::Opaque { .. })
                && self
                    .locations
                    .get(&node.id)
                    .and_then(|location| location.range.as_ref())
                    .is_some_and(|range| contains(&target, range))
        }) {
            return Err(CustomError::InvalidEdit("the replaced expression contains opaque Rust; edit that snippet in the source first".into()));
        }
        if let Some(binding) = from_location.bindings.iter().find(|binding| {
            binding.visible_after <= target.start
                && !self.locations.values().any(|location| {
                    location.bindings.iter().any(|other| {
                        other.name == binding.name
                            && other.visible_after > binding.visible_after
                            && other.visible_after <= target.start
                    })
                })
        }) {
            let replacement = with_comments(&self.source[target.clone()], &binding.name);
            return self.apply(vec![(target, replacement)]);
        }

        let original = from_location.range.clone().ok_or_else(|| {
            CustomError::InvalidEdit("the source node has no reusable Rust expression".into())
        })?;
        let insertion = from_location
            .statement_start
            .min(to_location.statement_start);
        if from_location
            .references
            .values()
            .any(|visible_after| *visible_after > insertion)
        {
            return Err(CustomError::InvalidEdit(
                "this connection would use a local variable before its declaration".into(),
            ));
        }
        // A supported-looking parent of opaque Rust is not a safe expression
        // to lift: the preview cannot establish its scalar type or purity.
        if self.has_opaque_dependency(from_id) {
            return Err(CustomError::InvalidEdit(
                "the source expression depends on opaque Rust".into(),
            ));
        }
        let mut counter = 1;
        let name = loop {
            let candidate = format!("graph_value_{counter}");
            if !self.source.contains(&candidate) {
                break candidate;
            }
            counter += 1;
        };
        let indent = indentation(&self.source, insertion);
        let declaration = format!(
            "let {name}: f64 = {};\n{indent}",
            &self.source[original.clone()]
        );
        let replacement = with_comments(&self.source[target.clone()], &name);
        let mut edits = vec![
            (insertion..insertion, declaration),
            (target.clone(), replacement),
        ];
        if !matches!(from.kind, CodeNodeKind::Input { .. }) && !contains(&target, &original) {
            edits.push((original, name)); // Original comments move with the expression above.
        }
        self.apply(edits)
    }

    /// Evaluate only the documented scalar subset. This is an interpreter,
    /// not an arbitrary Rust executor; unknown statements are never skipped.
    pub fn evaluate(&self, inputs: &BTreeMap<String, f64>) -> Result<f64, CustomError> {
        if !self.unsupported.is_empty() {
            return Err(CustomError::Unsupported(self.unsupported.join("; ")));
        }
        self.evaluate_node("output", inputs, &mut BTreeMap::new(), &mut BTreeSet::new())
    }

    fn node(&self, id: &str) -> Result<&CodeNode, CustomError> {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .ok_or_else(|| CustomError::InvalidEdit(format!("unknown node `{id}`")))
    }

    fn location(&self, id: &str) -> Result<&NodeSource, CustomError> {
        self.locations
            .get(id)
            .ok_or_else(|| CustomError::InvalidEdit(format!("unknown node `{id}`")))
    }

    fn reaches(&self, start: &str, wanted: &str) -> bool {
        let mut todo = vec![start];
        let mut seen = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if id == wanted {
                return true;
            }
            if seen.insert(id) {
                todo.extend(
                    self.edges
                        .iter()
                        .filter(|edge| edge.from == id)
                        .map(|edge| edge.to.as_str()),
                );
            }
        }
        false
    }

    fn has_opaque_dependency(&self, id: &str) -> bool {
        let mut todo = vec![id];
        let mut seen = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            if self
                .nodes
                .iter()
                .any(|node| node.id == id && matches!(node.kind, CodeNodeKind::Opaque { .. }))
            {
                return true;
            }
            todo.extend(
                self.edges
                    .iter()
                    .filter(|edge| edge.to == id)
                    .map(|edge| edge.from.as_str()),
            );
        }
        false
    }

    fn apply(&mut self, mut edits: Vec<(Range<usize>, String)>) -> Result<(), CustomError> {
        edits.sort_by(|left, right| {
            left.0
                .start
                .cmp(&right.0.start)
                .then(left.0.end.cmp(&right.0.end))
        });
        for pair in edits.windows(2) {
            if pair[0].0.end > pair[1].0.start {
                return Err(CustomError::InvalidEdit("source edits overlap".into()));
            }
        }
        let mut candidate = self.source.clone();
        for (range, replacement) in edits.into_iter().rev() {
            if range.end > candidate.len()
                || !candidate.is_char_boundary(range.start)
                || !candidate.is_char_boundary(range.end)
            {
                return Err(CustomError::InvalidEdit(
                    "source span is not a UTF-8 boundary".into(),
                ));
            }
            candidate.replace_range(range, &replacement);
        }
        let parsed = Self::parse(&candidate)?;
        *self = parsed;
        Ok(())
    }

    fn evaluate_node(
        &self,
        id: &str,
        inputs: &BTreeMap<String, f64>,
        cache: &mut BTreeMap<String, f64>,
        visiting: &mut BTreeSet<String>,
    ) -> Result<f64, CustomError> {
        if let Some(value) = cache.get(id) {
            return Ok(*value);
        }
        if !visiting.insert(id.into()) {
            return Err(CustomError::Evaluation(
                "cycle in Rust expression graph".into(),
            ));
        }
        let node = self.node(id)?;
        let argument =
            |index: usize, cache: &mut BTreeMap<String, f64>, visiting: &mut BTreeSet<String>| {
                let edge = self
                    .edges
                    .iter()
                    .find(|edge| edge.to == id && edge.input == index)
                    .ok_or_else(|| {
                        CustomError::Evaluation(format!("missing input {index} on `{id}`"))
                    })?;
                self.evaluate_node(&edge.from, inputs, cache, visiting)
            };
        let value = match &node.kind {
            CodeNodeKind::Input { name } => *inputs
                .get(name)
                .ok_or_else(|| CustomError::MissingInput(name.clone()))?,
            CodeNodeKind::Number { value } => *value,
            CodeNodeKind::Binary { op } => {
                let left = argument(0, cache, visiting)?;
                let right = argument(1, cache, visiting)?;
                match op.as_str() {
                    "+" => left + right,
                    "-" => left - right,
                    "*" => left * right,
                    "/" => left / right,
                    "%" => left % right,
                    _ => return Err(CustomError::Unsupported(format!("operator `{op}`"))),
                }
            }
            CodeNodeKind::Call { name } => {
                let mut args = Vec::new();
                for index in 0..self.location(id)?.inputs.len() {
                    args.push(argument(index, cache, visiting)?);
                }
                evaluate_call(name, &args)?
            }
            CodeNodeKind::Output => argument(0, cache, visiting)?,
            CodeNodeKind::Opaque { .. } => {
                return Err(CustomError::Unsupported(format!("opaque node `{id}`")));
            }
        };
        visiting.remove(id);
        if !value.is_finite() {
            return Err(CustomError::Evaluation(format!(
                "`{}` produced a non-finite f64",
                node.label
            )));
        }
        cache.insert(id.into(), value);
        Ok(value)
    }
}

/// Compiles as ordinary Rust and demonstrates shared `let` values and calls.
pub fn demo_source() -> String {
    "// Custom speed is ordinary Rust.\nfn speed(time: f64, base: f64) -> f64 {\n    let wave = time.sin(); // retained when graph nodes are edited\n    let scale = wave * 0.25;\n    base + scale\n}\n".into()
}

struct SourceMap<'a> {
    source: &'a str,
    lines: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    fn new(source: &'a str) -> Self {
        // syn::parse_file strips a leading BOM before it computes token spans.
        // Later lines still use their original byte offsets.
        let mut lines = vec![if source.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        }];
        lines.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
        Self { source, lines }
    }

    fn range(&self, span: Span) -> Result<Range<usize>, CustomError> {
        let start = span.start();
        let end = span.end();
        let offset = |line: usize, column: usize| -> Option<usize> {
            // proc_macro2 columns count Unicode scalar values, not UTF-8 bytes.
            let base = *self.lines.get(line.checked_sub(1)?)?;
            let remaining = &self.source[base..];
            let bytes = remaining
                .char_indices()
                .nth(column)
                .map(|(index, _)| index)
                .or_else(|| (remaining.chars().count() == column).then_some(remaining.len()))?;
            Some(base + bytes)
        };
        let start = offset(start.line, start.column)
            .ok_or_else(|| CustomError::Parse("invalid source span start".into()))?;
        let end = offset(end.line, end.column)
            .ok_or_else(|| CustomError::Parse("invalid source span end".into()))?;
        Ok(start..end)
    }
}

#[derive(Clone)]
struct ExprRef {
    id: String,
    range: Range<usize>,
    references: BTreeMap<String, usize>,
}

struct Builder<'a> {
    map: SourceMap<'a>,
    program: CustomProgram,
    environment: BTreeMap<String, (String, usize)>,
    statement_start: usize,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str, function_name: String) -> Self {
        Self {
            map: SourceMap::new(source),
            program: CustomProgram {
                source: source.into(),
                function_name,
                nodes: vec![],
                edges: vec![],
                locations: BTreeMap::new(),
                unsupported: vec![],
                scalar_signature: true,
            },
            environment: BTreeMap::new(),
            statement_start: 0,
        }
    }

    fn function(&mut self, function: &ItemFn) -> Result<(), CustomError> {
        self.statement_start = function
            .block
            .stmts
            .first()
            .map(|statement| self.map.range(statement.span()).map(|range| range.start))
            .transpose()?
            .unwrap_or(self.map.range(function.block.span())?.start + 1);
        if function.sig.asyncness.is_some()
            || function.sig.constness.is_some()
            || function.sig.unsafety.is_some()
            || function.sig.abi.is_some()
            || !function.sig.generics.params.is_empty()
            || function.sig.generics.where_clause.is_some()
            || function.sig.variadic.is_some()
        {
            self.program.scalar_signature = false;
            self.program
                .unsupported
                .push("only a plain non-generic scalar function is previewable".into());
        }
        if function
            .attrs
            .iter()
            .any(|attribute| !attribute.path().is_ident("doc"))
        {
            self.program.scalar_signature = false;
            self.program
                .unsupported
                .push("function attributes other than documentation are not interpreted".into());
        }
        if !matches!(&function.sig.output, ReturnType::Type(_, ty) if is_f64(ty)) {
            self.program.scalar_signature = false;
            self.program
                .unsupported
                .push("the return type must be f64".into());
        }
        for arg in &function.sig.inputs {
            let FnArg::Typed(arg) = arg else {
                self.program.scalar_signature = false;
                self.program
                    .unsupported
                    .push("a receiver argument is unsupported".into());
                continue;
            };
            if let Pat::Ident(pattern) = &*arg.pat
                && arg.attrs.is_empty()
                && pattern.by_ref.is_none()
                && pattern.subpat.is_none()
                && is_f64(&arg.ty)
            {
                let name = pattern.ident.to_string();
                let range = self.map.range(pattern.ident.span())?;
                let expression = self.add(
                    CodeNodeKind::Input { name: name.clone() },
                    name.clone(),
                    range,
                    vec![],
                    None,
                    true,
                );
                self.program
                    .locations
                    .get_mut(&expression.id)
                    .unwrap()
                    .bindings
                    .push(Binding {
                        name: name.clone(),
                        visible_after: 0,
                    });
                self.environment.insert(name, (expression.id, 0));
                continue;
            }
            self.program.scalar_signature = false;
            self.opaque(arg.span(), "only named f64 arguments are previewable")?;
        }

        let mut output = None;
        for (index, statement) in function.block.stmts.iter().enumerate() {
            self.statement_start = self.map.range(statement.span())?.start;
            let last = index + 1 == function.block.stmts.len();
            match statement {
                Stmt::Local(local) => {
                    if !local.attrs.is_empty() {
                        self.opaque(
                            local.span(),
                            "attributes on local declarations are not interpreted",
                        )?;
                        continue;
                    }
                    let (pattern, annotated_scalar) = match &local.pat {
                        Pat::Type(pattern) => (&*pattern.pat, is_f64(&pattern.ty)),
                        pattern => (pattern, true),
                    };
                    if let (Pat::Ident(pattern), Some(initializer)) = (pattern, &local.init)
                        && annotated_scalar
                        && pattern.by_ref.is_none()
                        && pattern.subpat.is_none()
                        && initializer.diverge.is_none()
                    {
                        let expression = self.expression(&initializer.expr)?;
                        let name = pattern.ident.to_string();
                        let visible_after = self.map.range(local.span())?.end;
                        self.program
                            .locations
                            .get_mut(&expression.id)
                            .unwrap()
                            .bindings
                            .push(Binding {
                                name: name.clone(),
                                visible_after,
                            });
                        self.environment
                            .insert(name, (expression.id, visible_after));
                        continue;
                    }
                    self.opaque(
                        local.span(),
                        "destructuring, non-f64 annotations, and let-else are unsupported",
                    )?;
                }
                Stmt::Expr(Expr::Return(returned), _) if last => {
                    if !returned.attrs.is_empty() {
                        self.opaque(returned.span(), "return attributes are not interpreted")?;
                    } else if let Some(expr) = &returned.expr {
                        output = Some(self.expression(expr)?);
                    } else {
                        self.opaque(returned.span(), "a scalar return requires an expression")?;
                    }
                }
                Stmt::Expr(expr, None) if last => output = Some(self.expression(expr)?),
                Stmt::Expr(expr, _) => {
                    self.opaque(
                        expr.span(),
                        "statements with side effects or early returns are unsupported",
                    )?;
                }
                Stmt::Item(item) => {
                    self.opaque(item.span(), "items inside the function are unsupported")?;
                }
                Stmt::Macro(mac) => {
                    self.opaque(mac.span(), "macros are unsupported")?;
                }
            }
        }
        if let Some(expression) = output {
            self.add_with_id(
                "output".into(),
                CodeNodeKind::Output,
                "Return f64".into(),
                expression.range.clone(),
                vec![expression],
                None,
                true,
            );
        } else {
            self.program
                .unsupported
                .push("the function has no supported final return expression".into());
            let range = self.map.range(function.block.span())?;
            self.add_with_id(
                "output".into(),
                CodeNodeKind::Output,
                "Return (unavailable)".into(),
                range,
                vec![],
                None,
                false,
            );
        }
        Ok(())
    }

    fn expression(&mut self, expr: &Expr) -> Result<ExprRef, CustomError> {
        let range = self.map.range(expr.span())?;
        if self.map.source[range.clone()].trim_start().starts_with('#') {
            return self.opaque(expr.span(), "expression attributes are not interpreted");
        }
        match expr {
            Expr::Paren(paren) => {
                let mut expression = self.expression(&paren.expr)?;
                expression.range = range;
                Ok(expression)
            }
            Expr::Group(group) => {
                let mut expression = self.expression(&group.expr)?;
                expression.range = range;
                Ok(expression)
            }
            Expr::Lit(literal) => {
                if let Lit::Float(float) = &literal.lit
                    && (float.suffix().is_empty() || float.suffix() == "f64")
                    && let Ok(value) = float.base10_parse::<f64>()
                    && value.is_finite()
                {
                    return Ok(self.add(
                        CodeNodeKind::Number { value },
                        value.to_string(),
                        range,
                        vec![],
                        None,
                        true,
                    ));
                }
                self.opaque(
                    expr.span(),
                    "only finite f64 floating-point literals are supported",
                )
            }
            Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
                let name = path.path.segments[0].ident.to_string();
                if let Some((id, visible_after)) = self.environment.get(&name) {
                    Ok(ExprRef {
                        id: id.clone(),
                        range,
                        references: BTreeMap::from([(name, *visible_after)]),
                    })
                } else {
                    self.opaque(
                        expr.span(),
                        "unresolved names or non-scalar constants are unsupported",
                    )
                }
            }
            Expr::Binary(binary) => {
                if let Some(op) = binary_operator(&binary.op) {
                    let left = self.expression(&binary.left)?;
                    let right = self.expression(&binary.right)?;
                    let operator = self.map.range(binary.op.span())?;
                    Ok(self.add(
                        CodeNodeKind::Binary { op: op.into() },
                        op.into(),
                        range,
                        vec![left, right],
                        Some(operator),
                        true,
                    ))
                } else {
                    self.opaque(
                        expr.span(),
                        "only f64 arithmetic binary operators are supported",
                    )
                }
            }
            Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => {
                if let Expr::Lit(literal) = &*unary.expr
                    && let Lit::Float(float) = &literal.lit
                    && (float.suffix().is_empty() || float.suffix() == "f64")
                    && let Ok(number) = float.base10_parse::<f64>()
                    && number.is_finite()
                {
                    return Ok(self.add(
                        CodeNodeKind::Number { value: -number },
                        (-number).to_string(),
                        range,
                        vec![],
                        None,
                        true,
                    ));
                }
                let arg = self.expression(&unary.expr)?;
                Ok(self.add(
                    CodeNodeKind::Call { name: "neg".into() },
                    "Negate".into(),
                    range,
                    vec![arg],
                    None,
                    true,
                ))
            }
            Expr::MethodCall(call) => {
                let name = call.method.to_string();
                let count = call.args.len() + 1;
                if call.turbofish.is_none() && call_arity(&name) == Some(count) && name != "neg" {
                    let mut args = vec![self.expression(&call.receiver)?];
                    for arg in &call.args {
                        args.push(self.expression(arg)?);
                    }
                    Ok(self.add(
                        CodeNodeKind::Call { name: name.clone() },
                        name,
                        range,
                        args,
                        None,
                        true,
                    ))
                } else {
                    self.opaque(expr.span(), "unsupported method or method arity")
                }
            }
            Expr::Call(call) => {
                if let Expr::Path(path) = &*call.func {
                    let segments: Vec<_> = path
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.ident.to_string())
                        .collect();
                    let name = segments.last().cloned().unwrap_or_default();
                    if path.qself.is_none()
                        && segments.len() == 2
                        && segments[0] == "f64"
                        && path
                            .path
                            .segments
                            .iter()
                            .all(|segment| matches!(segment.arguments, syn::PathArguments::None))
                        && call_arity(&name) == Some(call.args.len())
                        && name != "neg"
                    {
                        let mut args = vec![];
                        for arg in &call.args {
                            args.push(self.expression(arg)?);
                        }
                        return Ok(self.add(
                            CodeNodeKind::Call { name: name.clone() },
                            name,
                            range,
                            args,
                            None,
                            true,
                        ));
                    }
                }
                self.opaque(
                    expr.span(),
                    "only documented f64 associated functions are supported",
                )
            }
            _ => self.opaque(
                expr.span(),
                "this Rust expression is outside the scalar preview subset",
            ),
        }
    }

    fn opaque(&mut self, span: Span, reason: &str) -> Result<ExprRef, CustomError> {
        let range = self.map.range(span)?;
        let source = self.map.source[range.clone()].to_string();
        let line = span.start().line;
        self.program
            .unsupported
            .push(format!("line {line}: {reason}"));
        Ok(self.add(
            CodeNodeKind::Opaque { source },
            format!("Opaque Rust (line {line})"),
            range,
            vec![],
            None,
            false,
        ))
    }

    fn add(
        &mut self,
        kind: CodeNodeKind,
        label: String,
        range: Range<usize>,
        inputs: Vec<ExprRef>,
        operator: Option<Range<usize>>,
        scalar: bool,
    ) -> ExprRef {
        let id = format!("n{}", self.program.nodes.len() + 1);
        self.add_with_id(id, kind, label, range, inputs, operator, scalar)
    }

    #[allow(clippy::too_many_arguments)] // Private construction of a node and its source span metadata.
    fn add_with_id(
        &mut self,
        id: String,
        kind: CodeNodeKind,
        label: String,
        range: Range<usize>,
        inputs: Vec<ExprRef>,
        operator: Option<Range<usize>>,
        scalar: bool,
    ) -> ExprRef {
        let mut references = BTreeMap::new();
        let mut depth = 0;
        for (index, input) in inputs.iter().enumerate() {
            references.extend(input.references.clone());
            depth = depth.max(self.program.locations[&input.id].depth + 1);
            self.program.edges.push(CodeEdge {
                from: input.id.clone(),
                to: id.clone(),
                input: index,
            });
        }
        self.program.nodes.push(CodeNode {
            id: id.clone(),
            label,
            kind,
            position: [
                20.0 + depth as f32 * 155.0,
                35.0 + self.program.nodes.len() as f32 * 65.0,
            ],
        });
        let scalar = scalar
            && inputs
                .iter()
                .all(|input| self.program.locations[&input.id].scalar);
        self.program.locations.insert(
            id.clone(),
            NodeSource {
                range: Some(range.clone()),
                inputs: inputs.iter().map(|input| input.range.clone()).collect(),
                operator,
                references: references.clone(),
                bindings: vec![],
                statement_start: self.statement_start,
                depth,
                scalar,
            },
        );
        ExprRef {
            id,
            range,
            references,
        }
    }
}

fn is_f64(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident("f64"))
}

fn binary_operator(op: &BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add(_) => Some("+"),
        BinOp::Sub(_) => Some("-"),
        BinOp::Mul(_) => Some("*"),
        BinOp::Div(_) => Some("/"),
        BinOp::Rem(_) => Some("%"),
        _ => None,
    }
}

fn call_arity(name: &str) -> Option<usize> {
    match name {
        "neg" | "sin" | "cos" | "tan" | "abs" | "sqrt" | "exp" | "ln" | "floor" | "ceil"
        | "round" => Some(1),
        "min" | "max" | "powf" | "atan2" => Some(2),
        "clamp" => Some(3),
        _ => None,
    }
}

fn evaluate_call(name: &str, args: &[f64]) -> Result<f64, CustomError> {
    if call_arity(name) != Some(args.len()) {
        return Err(CustomError::Unsupported(format!("call `{name}`")));
    }
    let x = args[0];
    Ok(match name {
        "neg" => -x,
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "abs" => x.abs(),
        "sqrt" => x.sqrt(),
        "exp" => x.exp(),
        "ln" => x.ln(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        "min" => x.min(args[1]),
        "max" => x.max(args[1]),
        "powf" => x.powf(args[1]),
        "atan2" => x.atan2(args[1]),
        "clamp" => {
            if args[1] > args[2] {
                return Err(CustomError::Evaluation(
                    "clamp minimum exceeds maximum".into(),
                ));
            }
            x.clamp(args[1], args[2])
        }
        _ => return Err(CustomError::Unsupported(format!("call `{name}`"))),
    })
}

fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && outer.end >= inner.end
}

fn indentation(source: &str, position: usize) -> String {
    let start = source[..position].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[start..position];
    if prefix.chars().all(char::is_whitespace) {
        prefix.into()
    } else {
        "    ".into()
    }
}

/// Supported scalar expressions contain no string literals, so comment
/// scanning here cannot confuse a Rust string with comment delimiters.
fn with_comments(original: &str, replacement: &str) -> String {
    let bytes = original.as_bytes();
    let mut comments = vec![];
    let mut index = 0;
    while index + 1 < bytes.len() {
        let start = index;
        if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            comments.push(&original[start..index]);
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1;
            while index + 1 < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            comments.push(&original[start..index]);
        } else {
            index += 1;
        }
    }
    if comments.is_empty() {
        replacement.into()
    } else {
        format!("({}\n{replacement})", comments.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(time: f64, base: f64) -> BTreeMap<String, f64> {
        BTreeMap::from([("time".into(), time), ("base".into(), base)])
    }

    fn find(program: &CustomProgram, predicate: impl Fn(&CodeNodeKind) -> bool) -> String {
        program
            .nodes()
            .iter()
            .find(|node| predicate(&node.kind))
            .unwrap()
            .id
            .clone()
    }

    #[test]
    fn parses_and_evaluates_shared_let_values() {
        let source = "fn speed(time: f64, base: f64) -> f64 { let x = time * 2.0; x + x + base }";
        let program = CustomProgram::parse(source).unwrap();
        assert_eq!(program.source(), source);
        assert_eq!(program.evaluate(&inputs(3.0, 1.0)).unwrap(), 13.0);
        let multiply = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "*"),
        );
        assert_eq!(
            program
                .edges()
                .iter()
                .filter(|edge| edge.from == multiply)
                .count(),
            2
        );
    }

    #[test]
    fn edits_literals_and_operators_without_reformatting_or_losing_opaque_code() {
        let source = "// untouched header\nfn helper() -> i32 { 7 }\nfn speed(time: f64, base: f64) -> f64 {\n    println!(\"opaque // string\"); // keep side effect\n    base /* between */ + - /* negative */ 2.0 // trailing\n}\n";
        let mut program = CustomProgram::parse(source).unwrap();
        let number = find(&program, |kind| matches!(kind, CodeNodeKind::Number { .. }));
        program.set_number(&number, 3.5).unwrap();
        let op = find(&program, |kind| matches!(kind, CodeNodeKind::Binary { .. }));
        program.set_binary_op(&op, "*").unwrap();
        assert!(
            program
                .source()
                .contains("println!(\"opaque // string\"); // keep side effect")
        );
        for comment in [
            "// untouched header",
            "/* between */",
            "/* negative */",
            "// trailing",
        ] {
            assert!(program.source().contains(comment));
        }
        assert!(program.source().contains("fn helper() -> i32 { 7 }"));
        assert!(matches!(
            program.evaluate(&inputs(0.0, 1.0)),
            Err(CustomError::Unsupported(_))
        ));
        compile_and_run(program.source(), "", None);
    }

    #[test]
    fn reconnects_bound_values_and_compiles_the_result() {
        let mut program = CustomProgram::parse(&demo_source()).unwrap();
        let base = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Input { name } if name == "base"),
        );
        let multiplier = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "*"),
        );
        program.connect(&base, &multiplier, 0).unwrap();
        assert!((program.evaluate(&inputs(0.0, 4.0)).unwrap() - 5.0).abs() < 1e-10);
        compile_and_run(program.source(), "assert_eq!(speed(0.0, 4.0), 5.0);", None);
    }

    #[test]
    fn reconnects_unbound_expression_as_a_shared_local_instead_of_a_copy() {
        let mut program =
            CustomProgram::parse("fn speed(time: f64, base: f64) -> f64 { (time * 2.0) + base }")
                .unwrap();
        let number = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Number { value } if *value == 2.0),
        );
        let add = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "+"),
        );
        program.connect(&number, &add, 1).unwrap();
        assert!(program.source().contains("let graph_value_1: f64 = 2.0;"));
        assert_eq!(
            program
                .nodes()
                .iter()
                .filter(|node| matches!(node.kind, CodeNodeKind::Number { .. }))
                .count(),
            1
        );
        assert_eq!(program.evaluate(&inputs(3.0, 8.0)).unwrap(), 8.0);
        compile_and_run(program.source(), "assert_eq!(speed(3.0, 8.0), 8.0);", None);
    }

    #[test]
    fn graph_rewire_keeps_comments_inside_the_replaced_operand() {
        let mut program = CustomProgram::parse(
            "fn speed(time: f64, base: f64) -> f64 { base + (time /* block */ * // line\n2.0) }",
        )
        .unwrap();
        let base = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Input { name } if name == "base"),
        );
        let add = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "+"),
        );
        program.connect(&base, &add, 1).unwrap();
        assert!(program.source().contains("/* block */"));
        assert!(program.source().contains("// line"));
        assert_eq!(program.evaluate(&inputs(1.0, 2.0)).unwrap(), 4.0);
        compile_and_run(program.source(), "assert_eq!(speed(1.0, 2.0), 4.0);", None);
    }

    #[test]
    fn rejects_cycles_unknown_types_missing_ports_and_forward_references_transactionally() {
        let mut program = CustomProgram::parse(
            "fn speed(time: f64, base: f64) -> f64 { let a = time * 2.0; let b = a + base; b }",
        )
        .unwrap();
        let multiply = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "*"),
        );
        let add = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "+"),
        );
        let original = program.source().to_string();
        assert!(program.connect(&add, &multiply, 0).is_err());
        assert!(program.connect(&multiply, &add, 99).is_err());
        assert_eq!(program.source(), original);

        let mut program = CustomProgram::parse("fn speed(time: f64, base: f64) -> f64 { let a = time * 2.0; let x = base + 3.0; let b = x + 4.0; a + b }").unwrap();
        let multiply = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Binary { op } if op == "*"),
        );
        let b = program
            .nodes()
            .iter()
            .filter(|node| matches!(&node.kind, CodeNodeKind::Binary { op } if op == "+"))
            .nth(1)
            .unwrap()
            .id
            .clone();
        assert!(
            program
                .connect(&b, &multiply, 0)
                .unwrap_err()
                .to_string()
                .contains("before its declaration")
        );

        let mut opaque = CustomProgram::parse("fn speed(time: f64, base: f64) -> f64 { let x = if time > 0.0 { 1.0 } else { 2.0 }; x + base }").unwrap();
        let unknown = find(&opaque, |kind| matches!(kind, CodeNodeKind::Opaque { .. }));
        let add = find(&opaque, |kind| matches!(kind, CodeNodeKind::Binary { .. }));
        assert!(opaque.connect(&unknown, &add, 1).is_err());
        let base = find(
            &opaque,
            |kind| matches!(kind, CodeNodeKind::Input { name } if name == "base"),
        );
        assert!(opaque.connect(&base, &add, 0).is_err());
    }

    #[test]
    fn handles_unicode_spans_and_math_methods_and_associated_calls() {
        let mut program = CustomProgram::parse("// комментарий\nfn speed(time: f64, base: f64) -> f64 { /* ещё */ f64::max(-time, base.abs()) + 2.0 }").unwrap();
        let number = find(&program, |kind| matches!(kind, CodeNodeKind::Number { .. }));
        program.set_number(&number, 4.0).unwrap();
        assert!(program.source().contains("/* ещё */"));
        assert_eq!(program.evaluate(&inputs(1.0, -2.0)).unwrap(), 6.0);
        compile_and_run(program.source(), "assert_eq!(speed(1.0, -2.0), 6.0);", None);
    }

    #[test]
    fn supports_final_return_and_reports_missing_inputs_and_nonfinite_math() {
        let program =
            CustomProgram::parse("fn speed(time: f64, base: f64) -> f64 { return time / base; }")
                .unwrap();
        assert_eq!(program.evaluate(&inputs(6.0, 2.0)).unwrap(), 3.0);
        assert!(matches!(
            program.evaluate(&BTreeMap::new()),
            Err(CustomError::MissingInput(_))
        ));
        assert!(matches!(
            program.evaluate(&inputs(1.0, 0.0)),
            Err(CustomError::Evaluation(_))
        ));
        assert!(CustomProgram::parse("fn speed( { broken").is_err());
    }

    #[test]
    fn rewire_respects_shadowed_locals_and_parameters() {
        let mut program = CustomProgram::parse("fn speed(time: f64, mut base: f64) -> f64 { let old = 2.0; let old = 3.0; let base = 4.0; time + old + base }").unwrap();
        let original_parameter = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Input { name } if name == "base"),
        );
        let final_add = program
            .nodes()
            .iter()
            .rfind(|node| matches!(&node.kind, CodeNodeKind::Binary { op } if op == "+"))
            .unwrap()
            .id
            .clone();
        program.connect(&original_parameter, &final_add, 1).unwrap();
        assert_eq!(program.evaluate(&inputs(1.0, 10.0)).unwrap(), 14.0);
        compile_and_run(
            program.source(),
            "assert_eq!(speed(1.0, 10.0), 14.0);",
            None,
        );

        let first_old = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Number { value } if *value == 2.0),
        );
        let final_add = program
            .nodes()
            .iter()
            .rfind(|node| matches!(&node.kind, CodeNodeKind::Binary { op } if op == "+"))
            .unwrap()
            .id
            .clone();
        program.connect(&first_old, &final_add, 1).unwrap();
        assert_eq!(program.evaluate(&inputs(1.0, 10.0)).unwrap(), 6.0);
        compile_and_run(program.source(), "assert_eq!(speed(1.0, 10.0), 6.0);", None);
    }

    #[test]
    fn attributes_and_macro_blocks_are_retained_but_never_silently_interpreted() {
        let source = "#[cfg(any())]\nfn speed(time: f64, base: f64) -> f64 {\n    if true { println!(\"{time}\"); }\n    base + 2.0\n}";
        let mut program = CustomProgram::parse(source).unwrap();
        let number = find(&program, |kind| matches!(kind, CodeNodeKind::Number { .. }));
        program.set_number(&number, 3.0).unwrap();
        assert!(
            program
                .source()
                .contains("if true { println!(\"{time}\"); }")
        );
        assert!(program.source().contains("#[cfg(any())]"));
        assert!(matches!(
            program.evaluate(&inputs(0.0, 1.0)),
            Err(CustomError::Unsupported(_))
        ));
        compile_and_run(program.source(), "", None);

        let program = CustomProgram::parse(
            "fn speed(time: f64, base: f64) -> f64 { #[cfg(any())] let x = time; x + base }",
        )
        .unwrap();
        assert!(matches!(
            program.evaluate(&inputs(0.0, 1.0)),
            Err(CustomError::Unsupported(_))
        ));
    }

    #[test]
    fn bom_shebang_and_unicode_do_not_shift_source_edits() {
        for prefix in [
            "\u{feff}/* юникод */ ",
            "\u{feff}#!/usr/bin/env rust-script\n",
        ] {
            let source = format!("{prefix}fn speed(time:f64,base:f64)->f64{{ base + 2.0 }}");
            let mut program = CustomProgram::parse(&source).unwrap();
            let number = find(&program, |kind| matches!(kind, CodeNodeKind::Number { .. }));
            program.set_number(&number, 3.0).unwrap();
            assert_eq!(program.source(), source.replace("2.0", "3.0"));
            assert_eq!(program.evaluate(&inputs(0.0, 1.0)).unwrap(), 4.0);
            compile_and_run(program.source(), "assert_eq!(speed(0.0, 1.0), 4.0);", None);
        }
    }

    #[test]
    fn numeric_edits_preserve_inference_and_rewire_requires_a_scalar_signature() {
        let source = "fn consume(_x:f32) {}\nfn speed(time:f64,base:f64)->f64 { let x = 2.0; consume(x); base }";
        let mut program = CustomProgram::parse(source).unwrap();
        let number = find(&program, |kind| matches!(kind, CodeNodeKind::Number { .. }));
        program.set_number(&number, 3.5).unwrap();
        assert!(program.source().contains("let x = 3.5;"));
        compile_and_run(program.source(), "assert_eq!(speed(0.0, 1.0), 1.0);", None);

        let mut program = CustomProgram::parse("fn speed(time:f64,base:f64)->f32 { 2.0 }").unwrap();
        let base = find(
            &program,
            |kind| matches!(kind, CodeNodeKind::Input { name } if name == "base"),
        );
        assert!(program.connect(&base, "output", 0).is_err());
    }

    fn compile_and_run(source: &str, assertions: &str, extra: Option<&str>) {
        use std::process::Command;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "module_engine_custom_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("check.rs");
        let binary = path.join(if cfg!(windows) { "check.exe" } else { "check" });
        std::fs::write(
            &file,
            format!(
                "{source}\n{}\nfn main() {{ {assertions} }}",
                extra.unwrap_or("")
            ),
        )
        .unwrap();
        let output = Command::new("rustc")
            .arg("--edition=2024")
            .arg(&file)
            .arg("-o")
            .arg(&binary)
            .output()
            .expect("rustc must be installed for prototype compilation tests");
        assert!(
            output.status.success(),
            "generated Rust failed to compile:\n{}\n{}",
            source,
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "compiled preview disagreed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(&path).unwrap();
    }
}
