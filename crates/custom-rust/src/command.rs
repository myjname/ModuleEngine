//! Source-preserving operation graph for a compiled Rust update function.
//! Commands/literals are editable; unsupported Rust is retained as opaque text.
//! This representation is not an interpreter and never approximates execution.

use super::{CodeEdge, CustomError, SourceMap, with_comments};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range};
use syn::{Expr, FnArg, Item, Lit, Pat, Stmt, UnOp, spanned::Spanned};

/// A known portable operation or a literal feeding an operation parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CommandNodeKind {
    WorldCommand { method: String },
    RenderCommand { method: String },
    Number { value: f64 },
    Text { value: String },
    Boolean { value: bool },
    Opaque { source: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandNode {
    pub id: String,
    pub label: String,
    pub kind: CommandNodeKind,
    pub position: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
enum ParameterType {
    ObjectId,
    Byte,
    Float32,
    Float64,
    Bool,
    Text,
    Transform,
    WorldObject,
    Position3,
    Color4,
    Float4,
}

impl ParameterType {
    fn name(self) -> &'static str {
        match self {
            Self::ObjectId => "ObjectId",
            Self::Byte => "u8",
            Self::Float32 => "f32",
            Self::Float64 => "f64",
            Self::Bool => "bool",
            Self::Text => "String",
            Self::Transform => "Transform",
            Self::WorldObject => "WorldObject",
            Self::Position3 => "Position3",
            Self::Color4 => "Color4",
            Self::Float4 => "Float4",
        }
    }
}

#[derive(Clone, Debug)]
struct CommandLocation {
    range: Range<usize>,
    parameter_type: ParameterType,
    suffix: String,
}

/// Readable graph of top-level portable helper calls. Statement order stays in
/// the source; loops/conditions and arbitrary expressions remain opaque.
#[derive(Clone, Debug)]
pub struct CommandProgram {
    source: String,
    nodes: Vec<CommandNode>,
    edges: Vec<CodeEdge>,
    locations: BTreeMap<String, CommandLocation>,
    input_types: BTreeMap<String, Vec<String>>,
    output_types: BTreeMap<String, String>,
}

impl CommandProgram {
    pub fn parse(source: &str) -> Result<Self, CustomError> {
        let file = syn::parse_file(source).map_err(|e| CustomError::Parse(e.to_string()))?;
        let function = file
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn(f) if f.sig.ident == "update" => Some(f),
                _ => None,
            })
            .ok_or_else(|| {
                CustomError::Parse("expected fn update(ctx: &mut ScriptContext)".into())
            })?;
        let receiver = function
            .sig
            .inputs
            .first()
            .and_then(|a| match a {
                FnArg::Typed(p) => match p.pat.as_ref() {
                    Pat::Ident(id) => Some(id.ident.to_string()),
                    _ => None,
                },
                _ => None,
            })
            .ok_or_else(|| {
                CustomError::Parse("update requires a named ScriptContext parameter".into())
            })?;
        let map = SourceMap::new(source);
        let mut program = Self {
            source: source.into(),
            nodes: vec![],
            edges: vec![],
            locations: BTreeMap::new(),
            input_types: BTreeMap::new(),
            output_types: BTreeMap::new(),
        };
        let mut cursor_y = 40.0;
        for item in &file.items {
            if !matches!(item,Item::Fn(f) if f.sig.ident=="update") {
                program.opaque(
                    map.range(item.span())?,
                    [40., cursor_y],
                    "Retained Rust item",
                );
                cursor_y += 140.0;
            }
        }
        let mut command_index = 0;
        let mut context_shadowed = false;
        for statement in &function.block.stmts {
            if let Stmt::Local(local) = statement
                && pattern_binds(&local.pat, &receiver)
            {
                context_shadowed = true;
            }
            let call = match statement {
                Stmt::Expr(Expr::MethodCall(call), _)
                    if !context_shadowed
                        && matches!(call.receiver.as_ref(),Expr::Path(p) if p.path.is_ident(&receiver)) =>
                {
                    Some(call)
                }
                _ => None,
            };
            let definition = call.and_then(|c| method_parameters(&c.method.to_string()));
            let Some((render, parameters)) = definition else {
                program.opaque(
                    map.range(statement.span())?,
                    [40., cursor_y],
                    "Opaque Rust statement",
                );
                command_index += 1;
                cursor_y += 140.0;
                continue;
            };
            let call = call.unwrap();
            if call.args.len() != parameters.len() {
                program.opaque(
                    map.range(statement.span())?,
                    [40., cursor_y],
                    "Command with unsupported argument count",
                );
                command_index += 1;
                cursor_y += 140.0;
                continue;
            }
            let command_id = format!("command_{command_index}");
            let parameter_count: usize = call
                .args
                .iter()
                .zip(&parameters)
                .map(|(argument, (_, kind))| match (kind, argument) {
                    (ParameterType::Position3, Expr::Array(a)) if a.elems.len() == 3 => 3,
                    (ParameterType::Color4 | ParameterType::Float4, Expr::Array(a))
                        if a.elems.len() == 4 =>
                    {
                        4
                    }
                    _ => 1,
                })
                .sum();
            let command_height = 62.0 + parameter_count.max(1) as f32 * 25.0;
            let group_height =
                ((parameter_count.saturating_sub(1)) as f32 * 110.0 + 87.0).max(command_height);
            let method = call.method.to_string();
            program.nodes.push(CommandNode {
                id: command_id.clone(),
                label: format!("{receiver}.{method}"),
                kind: if render {
                    CommandNodeKind::RenderCommand { method }
                } else {
                    CommandNodeKind::WorldCommand { method }
                },
                position: [390., cursor_y + (group_height - command_height) * 0.5],
            });
            program.output_types.insert(
                command_id.clone(),
                if render {
                    "RenderCommand"
                } else {
                    "WorldCommand"
                }
                .into(),
            );
            let mut slot = 0;
            for (argument, (label, kind)) in call.args.iter().zip(parameters) {
                let elements = match (kind, argument) {
                    (ParameterType::Position3, Expr::Array(a)) if a.elems.len() == 3 => {
                        Some((a.elems.iter().collect::<Vec<_>>(), ParameterType::Float32))
                    }
                    (ParameterType::Color4, Expr::Array(a)) if a.elems.len() == 4 => {
                        Some((a.elems.iter().collect::<Vec<_>>(), ParameterType::Byte))
                    }
                    (ParameterType::Float4, Expr::Array(a)) if a.elems.len() == 4 => {
                        Some((a.elems.iter().collect::<Vec<_>>(), ParameterType::Float32))
                    }
                    _ => None,
                };
                if let Some((elements, element_kind)) = elements {
                    for (index, element) in elements.into_iter().enumerate() {
                        program.parameter(
                            &map,
                            element,
                            &command_id,
                            slot,
                            &format!("{label}[{index}]"),
                            element_kind,
                            cursor_y,
                        )?;
                        slot += 1;
                    }
                } else {
                    program.parameter(&map, argument, &command_id, slot, label, kind, cursor_y)?;
                    slot += 1;
                }
            }
            command_index += 1;
            cursor_y += group_height + 60.0;
        }
        Ok(program)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn nodes(&self) -> &[CommandNode] {
        &self.nodes
    }
    pub fn edges(&self) -> &[CodeEdge] {
        &self.edges
    }
    pub fn node_output_type(&self, id: &str) -> Option<&str> {
        self.output_types.get(id).map(String::as_str)
    }
    pub fn node_input_types(&self, id: &str) -> &[String] {
        self.input_types.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Edit the actual Rust literal, respecting object-ID/color integer ranges.
    /// Unsupported expressions and unrepresentable IDs never become fake floats.
    pub fn set_number(&mut self, id: &str, value: f64) -> Result<(), CustomError> {
        if !value.is_finite() {
            return Err(CustomError::InvalidEdit("number must be finite".into()));
        }
        if !matches!(self.node(id)?.kind, CommandNodeKind::Number { .. }) {
            return Err(CustomError::InvalidEdit(
                "selected command parameter is not a number".into(),
            ));
        }
        let location = self
            .locations
            .get(id)
            .ok_or_else(|| CustomError::InvalidEdit("literal span missing".into()))?
            .clone();
        let replacement = match location.parameter_type {
            ParameterType::ObjectId => {
                if value < 0. || value.fract() != 0. || value > 9_007_199_254_740_991. {
                    return Err(CustomError::InvalidEdit(
                        "object ID must be an exactly representable nonnegative integer".into(),
                    ));
                }
                format!("{value:.0}{}", location.suffix)
            }
            ParameterType::Byte => {
                if !(0.0..=255.).contains(&value) || value.fract() != 0. {
                    return Err(CustomError::InvalidEdit(
                        "color channel must be an integer in 0..255".into(),
                    ));
                }
                format!("{value:.0}{}", location.suffix)
            }
            ParameterType::Float32 | ParameterType::Float64 => {
                if matches!(location.parameter_type, ParameterType::Float32)
                    && !(value as f32).is_finite()
                {
                    return Err(CustomError::InvalidEdit(
                        "number is outside f32 range".into(),
                    ));
                }
                format!("{value:?}{}", location.suffix)
            }
            _ => return Err(CustomError::InvalidEdit("parameter is not numeric".into())),
        };
        self.patch(location.range, replacement, true)
    }

    pub fn set_string(&mut self, id: &str, value: &str) -> Result<(), CustomError> {
        if !matches!(self.node(id)?.kind, CommandNodeKind::Text { .. }) {
            return Err(CustomError::InvalidEdit(
                "selected parameter is not text".into(),
            ));
        }
        let range = self.locations.get(id).unwrap().range.clone();
        self.patch(range, format!("{value:?}"), false)
    }
    pub fn set_boolean(&mut self, id: &str, value: bool) -> Result<(), CustomError> {
        if !matches!(self.node(id)?.kind, CommandNodeKind::Boolean { .. }) {
            return Err(CustomError::InvalidEdit(
                "selected parameter is not bool".into(),
            ));
        }
        let range = self.locations.get(id).unwrap().range.clone();
        self.patch(range, value.to_string(), true)
    }

    /// Command execution/control order is authored in Rust. Unsupported rewiring
    /// is an explicit error, never a graph-only edit disconnected from the source.
    pub fn connect(&mut self, _from: &str, _to: &str, _input: usize) -> Result<(), CustomError> {
        Err(CustomError::InvalidEdit(
            "command parameter rewiring is not implemented; edit its literal or Rust expression"
                .into(),
        ))
    }

    fn node(&self, id: &str) -> Result<&CommandNode, CustomError> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| CustomError::InvalidEdit(format!("unknown command node {id}")))
    }
    fn patch(
        &mut self,
        range: Range<usize>,
        value: String,
        keep_comments: bool,
    ) -> Result<(), CustomError> {
        // A string literal can contain // or /* as data, never as comments.
        let replacement = if keep_comments {
            with_comments(&self.source[range.clone()], &value)
        } else {
            value
        };
        let mut source = self.source.clone();
        source.replace_range(range, &replacement);
        let candidate = Self::parse(&source)?;
        *self = candidate;
        Ok(())
    }
    fn opaque(&mut self, range: Range<usize>, position: [f32; 2], label: &str) -> String {
        let id = format!("opaque_{}", self.nodes.len());
        self.nodes.push(CommandNode {
            id: id.clone(),
            label: label.into(),
            kind: CommandNodeKind::Opaque {
                source: self.source[range].into(),
            },
            position,
        });
        self.output_types.insert(id.clone(), "UnknownRust".into());
        id
    }
    #[allow(clippy::too_many_arguments)] // Source span, typed slot and layout share one parser operation.
    fn parameter(
        &mut self,
        map: &SourceMap<'_>,
        expression: &Expr,
        command: &str,
        input: usize,
        label: &str,
        kind: ParameterType,
        group_y: f32,
    ) -> Result<(), CustomError> {
        let range = map.range(expression.span())?;
        let id = format!("{command}_parameter_{input}");
        let mut suffix = String::new();
        let literal = literal(expression, false);
        let node_kind = match (kind, literal) {
            (ParameterType::Text, Some((Lit::Str(s), false))) => {
                CommandNodeKind::Text { value: s.value() }
            }
            (ParameterType::Bool, Some((Lit::Bool(b), false))) => {
                CommandNodeKind::Boolean { value: b.value }
            }
            (
                ParameterType::ObjectId
                | ParameterType::Byte
                | ParameterType::Float32
                | ParameterType::Float64,
                Some((Lit::Int(i), negative)),
            ) => {
                suffix = i.suffix().into();
                let value = i
                    .base10_parse::<f64>()
                    .map_err(|e| CustomError::Parse(e.to_string()))?
                    * if negative { -1. } else { 1. };
                if matches!(kind, ParameterType::ObjectId) && value > 9_007_199_254_740_991. {
                    CommandNodeKind::Opaque {
                        source: self.source[range.clone()].into(),
                    }
                } else {
                    CommandNodeKind::Number { value }
                }
            }
            (ParameterType::Float32 | ParameterType::Float64, Some((Lit::Float(f), negative))) => {
                suffix = f.suffix().into();
                CommandNodeKind::Number {
                    value: f
                        .base10_parse::<f64>()
                        .map_err(|e| CustomError::Parse(e.to_string()))?
                        * if negative { -1. } else { 1. },
                }
            }
            _ => CommandNodeKind::Opaque {
                source: self.source[range.clone()].into(),
            },
        };
        let opaque = matches!(node_kind, CommandNodeKind::Opaque { .. });
        self.nodes.push(CommandNode {
            id: id.clone(),
            label: label.into(),
            kind: node_kind,
            position: [40., group_y + input as f32 * 110.0],
        });
        self.locations.insert(
            id.clone(),
            CommandLocation {
                range,
                parameter_type: kind,
                suffix,
            },
        );
        self.input_types
            .entry(command.into())
            .or_default()
            .push(kind.name().into());
        self.output_types.insert(
            id.clone(),
            if opaque {
                "UnknownRust".into()
            } else {
                kind.name().into()
            },
        );
        self.edges.push(CodeEdge {
            from: id,
            to: command.into(),
            input,
        });
        Ok(())
    }
}

fn literal(expression: &Expr, negative: bool) -> Option<(&Lit, bool)> {
    match expression {
        Expr::Lit(e) => Some((&e.lit, negative)),
        Expr::Paren(e) => literal(&e.expr, negative),
        Expr::Group(e) => literal(&e.expr, negative),
        Expr::Unary(e) if matches!(e.op, UnOp::Neg(_)) => literal(&e.expr, !negative),
        _ => None,
    }
}

fn pattern_binds(pattern: &Pat, name: &str) -> bool {
    match pattern {
        Pat::Ident(p) => {
            p.ident == name
                || p.subpat
                    .as_ref()
                    .is_some_and(|(_, p)| pattern_binds(p, name))
        }
        Pat::Tuple(p) => p.elems.iter().any(|p| pattern_binds(p, name)),
        Pat::TupleStruct(p) => p.elems.iter().any(|p| pattern_binds(p, name)),
        Pat::Struct(p) => p.fields.iter().any(|f| pattern_binds(&f.pat, name)),
        Pat::Reference(p) => pattern_binds(&p.pat, name),
        Pat::Type(p) => pattern_binds(&p.pat, name),
        Pat::Paren(p) => pattern_binds(&p.pat, name),
        Pat::Slice(p) => p.elems.iter().any(|p| pattern_binds(p, name)),
        _ => false,
    }
}

fn method_parameters(method: &str) -> Option<(bool, Vec<(&'static str, ParameterType)>)> {
    use ParameterType::*;
    Some(match method {
        "set_position" => (false, vec![("id", ObjectId), ("position", Position3)]),
        "translate" => (false, vec![("id", ObjectId), ("delta", Position3)]),
        "set_transform" => (false, vec![("id", ObjectId), ("transform", Transform)]),
        "set_color" => (false, vec![("id", ObjectId), ("color", Color4)]),
        "spawn" => (false, vec![("object", WorldObject)]),
        "remove" => (false, vec![("id", ObjectId)]),
        "set_render_parameter" => (
            true,
            vec![("pass", Text), ("name", Text), ("value", Float64)],
        ),
        "enable_render_pass" => (true, vec![("pass", Text), ("enabled", Bool)]),
        "clear_resource" => (true, vec![("name", Text), ("value", Float4)]),
        "copy_resource" => (true, vec![("from", Text), ("to", Text)]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_command_parameters_update_real_source_and_preserve_opaque_unicode_comments() {
        let source = "// Привет: untouched\nfn update(ctx:&mut ScriptContext) {\n let x = (ctx.time.sin()) as f32; // retained opaque logic\n ctx.set_position(1, [x, /* высота */ 0.25f32, 0.0]);\n ctx.set_color(1,[75,205,245,255]);\n ctx.set_render_parameter(\"tonemap\",\"exposure\",1.4);\n ctx.enable_render_pass(\"ao\", true);\n}\n";
        let mut program = CommandProgram::parse(source).unwrap();
        assert!(
            program
                .nodes
                .iter()
                .any(|n| matches!(n.kind, CommandNodeKind::WorldCommand { .. }))
        );
        assert!(
            program
                .nodes
                .iter()
                .any(|n| matches!(n.kind, CommandNodeKind::RenderCommand { .. }))
        );
        let id = program
            .nodes
            .iter()
            .find(|n| matches!(n.kind,CommandNodeKind::Number{value} if value==0.25))
            .unwrap()
            .id
            .clone();
        program.set_number(&id, 0.75).unwrap();
        assert!(program.source().contains("/* высота */ 0.75f32"));
        assert!(
            program
                .source()
                .contains("let x = (ctx.time.sin()) as f32; // retained opaque logic")
        );
        let id = program
            .nodes
            .iter()
            .find(|n| matches!(n.kind,CommandNodeKind::Number{value} if value==1.4))
            .unwrap()
            .id
            .clone();
        program.set_number(&id, 2.0).unwrap();
        assert!(program.source().contains("\"exposure\",2.0"));
        let id = program
            .nodes
            .iter()
            .find(|n| matches!(n.kind, CommandNodeKind::Boolean { .. }))
            .unwrap()
            .id
            .clone();
        program.set_boolean(&id, false).unwrap();
        assert!(program.source().contains("\"ao\", false"));
        let before = program.source().to_owned();
        let color = program
            .nodes
            .iter()
            .find(|n| matches!(n.kind,CommandNodeKind::Number{value} if value==205.))
            .unwrap()
            .id
            .clone();
        assert!(program.set_number(&color, 300.).is_err());
        assert_eq!(before, program.source());
    }
    #[test]
    fn text_with_comment_delimiters_and_negative_comments_remain_editable() {
        let mut program=CommandProgram::parse("fn update(ctx:&mut ScriptContext){ctx.copy_resource(\"a//b\",\"c/*d*/\");ctx.translate(1,[- /* keep */ 1.0,0.0,0.0]);}").unwrap();
        let text = program
            .nodes
            .iter()
            .find(|n| matches!(&n.kind,CommandNodeKind::Text{value} if value=="a//b"))
            .unwrap()
            .id
            .clone();
        program.set_string(&text, "x/*y*/").unwrap();
        assert!(program.source().contains("\"x/*y*/\""));
        let number = program
            .nodes
            .iter()
            .find(|n| matches!(n.kind,CommandNodeKind::Number{value} if value== -1.))
            .unwrap()
            .id
            .clone();
        program.set_number(&number, -2.).unwrap();
        program.set_number(&number, -3.).unwrap();
        assert!(program.source().contains("/* keep */"));
        assert!(
            program
                .nodes
                .iter()
                .any(|n| matches!(n.kind,CommandNodeKind::Number{value} if value== -3.))
        );
    }
    #[test]
    fn calls_after_context_shadowing_are_preserved_opaque() {
        let program = CommandProgram::parse(
            "fn update(ctx:&mut ScriptContext){let ctx=another_context();ctx.remove(1);}",
        )
        .unwrap();
        assert!(
            !program
                .nodes()
                .iter()
                .any(|n| matches!(n.kind, CommandNodeKind::WorldCommand { .. }))
        );
        assert_eq!(program.nodes().len(), 2);
    }
}
