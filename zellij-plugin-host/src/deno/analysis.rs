//! Parsed entrypoint analysis; nothing here executes guest code or follows imports for grants.
use super::{checker, loader};
use deno_ast::swc::{
    ast::*,
    common::{Span, SyntaxContext},
    ecma_visit::{Visit, VisitWith},
};
use deno_ast::{SourceRangedForSpanned, SourceTextInfo};
use std::{collections::HashSet, path::Path};
use zellij_types::{
    authoring::{OutlineEntry, OutlineKind, PluginCheckMode, PluginCheckReport},
    data::PermissionType,
};

pub(crate) fn check(path: &Path, source: &str, mode: PluginCheckMode) -> PluginCheckReport {
    let mut report = PluginCheckReport::default();
    let parsed = match loader::parse(path, source, true) {
        Ok(parsed) => parsed.into_with_scope_analysis(),
        Err(error) => {
            report.diagnostics.push(format!("{error:#}"));
            return report;
        },
    };
    let mut analysis = Analysis {
        text: parsed.text_info_lazy(),
        unresolved: parsed.unresolved_context(),
        sdk: HashSet::new(),
        namespaces: HashSet::new(),
        report: &mut report,
    };
    let program = parsed.program();
    // Resolve bindings before walking calls; imports are hoisted.
    if let Program::Module(module) = &*program {
        for item in &module.body {
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
                if import.type_only || import.src.value != loader::SDK_SPECIFIER {
                    continue;
                }
                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Default(specifier) => {
                            analysis.sdk.insert(specifier.local.to_id());
                        },
                        ImportSpecifier::Named(specifier)
                            if !specifier.is_type_only
                                && specifier
                                    .imported
                                    .as_ref()
                                    .map_or(specifier.local.sym.as_ref() == "zellij", |name| {
                                        name.atom().as_ref() == "zellij"
                                    }) =>
                        {
                            analysis.sdk.insert(specifier.local.to_id());
                        },
                        ImportSpecifier::Namespace(specifier) => {
                            analysis.namespaces.insert(specifier.local.to_id());
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    program.visit_with(&mut analysis);
    if report
        .outline
        .iter()
        .any(|entry| entry.kind == OutlineKind::Tool)
        && !report
            .permissions
            .contains(&PermissionType::MessageAndLaunchOtherPlugins)
    {
        report.warnings.push("Hocus tool registration requires MessageAndLaunchOtherPlugins; request it and wait for approval before calling zellij.hocus.tools".into());
    }
    report.outline.sort_by_key(|entry| entry.line);
    let result = match mode {
        PluginCheckMode::Syntax => checker::check_syntax(path, source),
        PluginCheckMode::Semantic => checker::check_semantic_source(path, source),
    };
    if let Err(error) = result {
        report.diagnostics.push(format!("{error:#}"));
    }
    report
}

struct Analysis<'a> {
    text: &'a SourceTextInfo,
    unresolved: SyntaxContext,
    sdk: HashSet<Id>,
    namespaces: HashSet<Id>,
    report: &'a mut PluginCheckReport,
}

fn literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(value)) => value.value.as_str().map(str::to_owned),
        _ => None,
    }
}
fn property(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.to_string()),
        MemberProp::Computed(prop) => literal(&prop.expr),
        _ => None,
    }
}
fn key(prop: &PropName) -> Option<String> {
    match prop {
        PropName::Ident(id) => Some(id.sym.to_string()),
        PropName::Str(value) => value.value.as_str().map(str::to_owned),
        _ => None,
    }
}

impl Analysis<'_> {
    fn line(&self, span: Span) -> usize {
        self.text.line_index(span.start()) + 1
    }
    fn entry(&mut self, span: Span, kind: OutlineKind, text: String) {
        self.report.outline.push(OutlineEntry {
            line: self.line(span),
            kind,
            text,
        });
    }
    fn sdk_object(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(id) => {
                self.sdk.contains(&id.to_id()) || (id.sym == "zellij" && id.ctxt == self.unresolved)
            },
            Expr::Member(member) => match &*member.obj {
                Expr::Ident(id) => {
                    (self.namespaces.contains(&id.to_id())
                        && matches!(
                            property(&member.prop).as_deref(),
                            Some("zellij" | "default")
                        ))
                        || (id.sym == "globalThis"
                            && id.ctxt == self.unresolved
                            && property(&member.prop).as_deref() == Some("zellij"))
                },
                _ => false,
            },
            _ => false,
        }
    }
    fn permissions(&mut self, span: Span, expr: Option<&Expr>) {
        let names: Option<Vec<String>> = match expr {
            Some(Expr::Array(array)) => array
                .elems
                .iter()
                .map(|item| {
                    item.as_ref()
                        .filter(|item| item.spread.is_none())
                        .and_then(|item| literal(&item.expr))
                })
                .collect(),
            _ => None,
        };
        let Some(names) = names else {
            self.report.warnings.push(format!(
                "line {}: computed permission request uses normal runtime approval",
                self.line(span)
            ));
            return;
        };
        self.entry(
            span,
            OutlineKind::Permission,
            format!("permissions: {}", names.join(", ")),
        );
        for name in names {
            match serde_json::from_value::<PermissionType>(serde_json::Value::String(name.clone()))
            {
                Ok(permission) if !self.report.permissions.contains(&permission) => {
                    self.report.permissions.push(permission)
                },
                Ok(_) => {},
                Err(_) => self.report.warnings.push(format!(
                    "line {}: unknown permission {name:?}",
                    self.line(span)
                )),
            }
        }
    }
}
impl Visit for Analysis<'_> {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        self.entry(
            import.span,
            OutlineKind::Require,
            format!("import {:?}", import.src.value),
        );
    }
    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.entry(
            function.function.span,
            OutlineKind::Function,
            format!("function {}", function.ident.sym),
        );
        function.visit_children_with(self);
    }
    fn visit_method_prop(&mut self, method: &MethodProp) {
        if let Some(name) = key(&method.key) {
            self.entry(
                method.function.span,
                OutlineKind::Function,
                format!("{name}(...)"),
            );
        }
        method.visit_children_with(self);
    }
    fn visit_var_declarator(&mut self, declaration: &VarDeclarator) {
        if matches!(
            declaration.init.as_deref(),
            Some(Expr::Arrow(_) | Expr::Fn(_))
        ) {
            if let Pat::Ident(id) = &declaration.name {
                self.entry(
                    declaration.span,
                    OutlineKind::Function,
                    format!("{}(...) ", id.sym),
                );
            }
        }
        declaration.visit_children_with(self);
    }
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = &**callee {
                if property(&member.prop).as_deref() == Some("tools") {
                    if let Expr::Member(hocus) = &*member.obj {
                        if self.sdk_object(&hocus.obj)
                            && property(&hocus.prop).as_deref() == Some("hocus")
                        {
                            self.entry(call.span, OutlineKind::Tool, "Hocus tools".into());
                            if let Some(Expr::Object(object)) = call
                                .args
                                .first()
                                .filter(|arg| arg.spread.is_none())
                                .map(|arg| &*arg.expr)
                            {
                                for prop in &object.props {
                                    if let PropOrSpread::Prop(prop) = prop {
                                        if let Prop::KeyValue(prop) = &**prop {
                                            if let Some(name) = key(&prop.key) {
                                                self.entry(call.span, OutlineKind::Tool, name);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if self.sdk_object(&member.obj) {
                    let argument = |index: usize| {
                        call.args
                            .get(index)
                            .filter(|arg| arg.spread.is_none())
                            .map(|arg| &*arg.expr)
                    };
                    match property(&member.prop).as_deref() {
                        Some("requestPermission" | "waitForPermissions") => {
                            self.permissions(call.span, argument(0))
                        },
                        Some("command") => match argument(0) {
                            Some(expr)
                                if literal(expr).as_deref() == Some("RequestPluginPermissions") =>
                            {
                                self.permissions(call.span, argument(1))
                            },
                            Some(Expr::Object(object)) => {
                                // A command object is exactly one own data field.
                                if let [PropOrSpread::Prop(prop)] = object.props.as_slice() {
                                    if let Prop::KeyValue(prop) = &**prop {
                                        if key(&prop.key).as_deref()
                                            == Some("RequestPluginPermissions")
                                        {
                                            self.permissions(call.span, Some(&prop.value));
                                        }
                                    }
                                }
                            },
                            _ => {},
                        },
                        Some("on" | "waitForEvent") => {
                            if let Some(name) = argument(0).and_then(literal) {
                                self.entry(call.span, OutlineKind::Handler, format!("on {name}"));
                            }
                        },
                        _ => {},
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}

#[cfg(test)]
#[path = "unit/analysis.rs"]
mod tests;
