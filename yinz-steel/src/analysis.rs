//! Static entrypoint inspection. Call only in a supervised compilation process.

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::HashSet;
use steel::parser::{ast::ExprKind, parser::Parser, tokens::TokenType};

#[derive(Default, Debug)]
pub struct Analysis {
    pub definitions: Vec<(usize, String)>,
    pub imports: Vec<(usize, String)>,
    pub commands: Vec<Command>,
}

#[derive(Debug)]
pub struct Command {
    pub line: usize,
    pub name: String,
    pub argument: Option<Value>,
    pub during_load: bool,
}

pub fn analyze(source: &str) -> Result<Analysis> {
    crate::validate_source(source)?;
    let expressions = Parser::new_flat(source, None)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("parse Scheme for analysis: {error}"))?;
    let mut analysis = Visitor {
        newlines: source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index))
            .collect(),
        result: Analysis::default(),
        mutated: HashSet::new(),
    };
    for expression in &expressions {
        mutations(expression, &mut analysis.mutated);
    }
    analysis.sequence(&expressions, &HashSet::new(), false);
    Ok(analysis.result)
}

fn items(expression: &ExprKind) -> &[ExprKind] {
    match expression {
        ExprKind::List(list) => &list.args,
        _ => &[],
    }
}

fn name(expression: &ExprKind) -> Option<String> {
    match expression {
        ExprKind::Atom(atom) => match &atom.syn.ty {
            TokenType::StringLiteral(_) => None,
            _ => Some(atom.syn.ty.to_string()),
        },
        _ => None,
    }
}

fn head(expressions: &[ExprKind]) -> Option<String> {
    expressions.first().and_then(name)
}

fn inert(head: &str) -> bool {
    matches!(
        head,
        "quote" | "quasiquote" | "syntax" | "quasisyntax" | "define-syntax" | "syntax-rules"
    )
}

fn mutations(expression: &ExprKind, out: &mut HashSet<String>) {
    let list = items(expression);
    if head(list).is_some_and(|name| inert(&name)) {
        return;
    }
    if head(list).as_deref() == Some("set!") {
        out.extend(list.get(1).and_then(name));
    }
    for child in list {
        mutations(child, out);
    }
}

fn bindings(expressions: &[ExprKind], bound: &mut HashSet<String>) {
    for expression in expressions {
        let list = items(expression);
        match head(list).as_deref() {
            Some("define" | "define-syntax") => {
                if let Some(target) = list.get(1) {
                    bound.extend(name(target).or_else(|| head(items(target))));
                }
            },
            Some("begin") => bindings(&list[1..], bound),
            Some("require") => {
                // An unrestricted import may replace these bindings; never infer extra grants.
                bound.extend(
                    ["yinz-command", "yinz-wait-for-permissions", "hash", "list"]
                        .map(str::to_owned),
                );
            },
            _ => {},
        }
    }
}

struct Visitor {
    newlines: Vec<usize>,
    result: Analysis,
    mutated: HashSet<String>,
}

impl Visitor {
    fn line(&self, expression: &ExprKind) -> usize {
        self.newlines
            .partition_point(|index| *index < expression.span().start as usize)
            + 1
    }

    fn sequence(&mut self, expressions: &[ExprKind], bound: &HashSet<String>, during_load: bool) {
        let mut scope = bound.clone();
        bindings(expressions, &mut scope);
        for expression in expressions {
            self.visit(expression, &scope, during_load);
        }
    }

    fn visit(&mut self, expression: &ExprKind, bound: &HashSet<String>, during_load: bool) {
        let list = items(expression);
        let Some(head) = head(list) else { return };
        if inert(&head) {
            return;
        }
        let line = self.line(expression);
        match head.as_str() {
            "define" if list.len() >= 3 => {
                let target = &list[1];
                self.result.definitions.push((line, target.to_string()));
                let function = items(target);
                if !function.is_empty() {
                    let mut scope = bound.clone();
                    scope.extend(function.iter().skip(1).filter_map(name));
                    self.sequence(&list[2..], &scope, head_name(function) == "load");
                    return;
                }
            },
            "lambda" if list.len() >= 3 => {
                let mut scope = bound.clone();
                scope.extend(name(&list[1]));
                scope.extend(items(&list[1]).iter().filter_map(name));
                self.sequence(&list[2..], &scope, during_load);
                return;
            },
            "let" | "let*" | "letrec" if list.len() >= 3 => {
                let mut scope = bound.clone();
                let binding_index = if let Some(name) = name(&list[1]) {
                    scope.insert(name);
                    2
                } else {
                    1
                };
                if let Some(binding_list) = list.get(binding_index) {
                    let pairs = items(binding_list);
                    scope.extend(pairs.iter().filter_map(|pair| self::head(items(pair))));
                    for pair in pairs {
                        self.sequence(
                            items(pair).get(1..).unwrap_or_default(),
                            &scope,
                            during_load,
                        );
                    }
                    self.sequence(&list[binding_index + 1..], &scope, during_load);
                }
                return;
            },
            "require" => self.result.imports.push((line, expression.to_string())),
            "yinz-wait-for-permissions"
                if !bound.contains(&head)
                    && !self.mutated.contains(&head)
                    && !bound.contains("yinz-command")
                    && !self.mutated.contains("yinz-command") =>
            {
                if let Some(argument) = list.get(1) {
                    self.result.commands.push(Command {
                        line,
                        name: "RequestPluginPermissions".into(),
                        argument: literal(argument, bound, &self.mutated, false),
                        during_load,
                    });
                }
            },
            "yinz-command" if !bound.contains(&head) && !self.mutated.contains(&head) => {
                if let Some(argument) = list.get(1) {
                    let command = argument
                        .to_string_literal()
                        .map(|name| (name, Some(Value::Null)))
                        .or_else(|| {
                            let hash = items(argument);
                            if head_name(hash) != "hash"
                                || hash.len() != 3
                                || bound.contains("hash")
                                || self.mutated.contains("hash")
                            {
                                return None;
                            }
                            Some((
                                hash[1].to_string_literal()?,
                                literal(&hash[2], bound, &self.mutated, false),
                            ))
                        });
                    if let Some((name, argument)) = command {
                        self.result.commands.push(Command {
                            line,
                            name,
                            argument,
                            during_load,
                        });
                    }
                }
            },
            _ => {},
        }
        for child in list.iter().skip(1) {
            self.visit(child, bound, during_load);
        }
    }
}

fn head_name(expressions: &[ExprKind]) -> String {
    head(expressions).unwrap_or_default()
}

fn literal(
    expression: &ExprKind,
    bound: &HashSet<String>,
    mutated: &HashSet<String>,
    quoted: bool,
) -> Option<Value> {
    if let Some(text) = expression.to_string_literal() {
        return Some(Value::String(text));
    }
    if let ExprKind::Atom(atom) = expression {
        return match &atom.syn.ty {
            TokenType::BooleanLiteral(value) => Some(Value::Bool(*value)),
            TokenType::Number(_) => serde_json::from_str(&atom.to_string()).ok(),
            TokenType::Identifier(identifier) if quoted && identifier.resolve() == "json-null" => {
                Some(Value::Null)
            },
            _ => None,
        };
    }
    if let ExprKind::Quote(quote) = expression {
        return literal(&quote.expr, bound, mutated, true);
    }
    let list = items(expression);
    if quoted {
        return list
            .iter()
            .map(|value| literal(value, bound, mutated, true))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array);
    }
    let head = head(list)?;
    match head.as_str() {
        "quote" if list.len() == 2 => literal(&list[1], bound, mutated, true),
        "list" if !bound.contains("list") && !mutated.contains("list") => list[1..]
            .iter()
            .map(|value| literal(value, bound, mutated, false))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        "hash" if !bound.contains("hash") && !mutated.contains("hash") && list.len() % 2 == 1 => {
            let object = list[1..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    Some((
                        pair[0].to_string_literal()?,
                        literal(&pair[1], bound, mutated, false)?,
                    ))
                })
                .collect::<Option<Map<String, Value>>>()?;
            Some(Value::Object(object))
        },
        _ => None,
    }
}

#[cfg(test)]
#[path = "unit/analysis.rs"]
mod tests;
