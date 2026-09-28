//! Parsed entrypoint analysis; nothing here executes guest code or follows imports for grants.
//! It runs in the compilation worker; `yinz_plugin_authoring::analysis::check` is its client.
use super::loader;
use deno_ast::swc::{
    ast::*,
    common::{comments::CommentKind, Span, SyntaxContext},
    ecma_visit::{Visit, VisitWith},
};
use deno_ast::{SourceRangedForSpanned, SourceTextInfo};
use std::{collections::HashSet, path::Path};
use yinz_types::{
    authoring::{OutlineEntry, OutlineKind, PluginCheckMode, PluginCheckReport},
    data::PermissionType,
    plugin_data::{validate_namespace, RESERVED_ATTR_NAMES, SHARED_NAMESPACE_PREFIX},
};

pub(super) fn check_authoring(
    path: &Path,
    source: &str,
    mode: PluginCheckMode,
) -> PluginCheckReport {
    let mut report = match super::worker::analyze(path, source) {
        Ok(report) => report,
        Err(error) => PluginCheckReport {
            diagnostics: vec![format!("{error:#}")],
            ..Default::default()
        },
    };
    if report.diagnostics.is_empty() && mode != PluginCheckMode::Syntax {
        match super::authoring::run(path, source, mode) {
            Ok(formatted) => {
                if let Some(ref source) = formatted {
                    match super::worker::analyze(path, source) {
                        Ok(formatted_report) => report = formatted_report,
                        Err(error) => report.diagnostics.push(format!("{error:#}")),
                    }
                }
                report.formatted_source = formatted;
            },
            Err(error) => report.diagnostics.push(format!("{error:#}")),
        }
    }
    report
}

// Worker-only: no semantic checker or subprocess calls can occur here.
pub(super) fn analyze_native(path: &Path, source: &str) -> PluginCheckReport {
    let mut report = PluginCheckReport::default();
    let parsed = match loader::parse(path, source, true) {
        Ok(parsed) => parsed.into_with_scope_analysis(),
        Err(error) => {
            report.diagnostics.push(format!("{error:#}"));
            return report;
        },
    };
    for comment in parsed.comments().get_vec() {
        let line_comment = comment.kind == CommentKind::Line;
        let text = comment.text.trim();
        let directive = text.trim_start_matches('*').trim();
        let lint_directive = [
            "eslint",
            "oxlint",
            "oxfmt-ignore",
            "prettier-ignore",
            "global ",
            "globals ",
            "exported ",
        ]
        .iter()
        .any(|prefix| directive.starts_with(prefix));
        // SWC removes the opening // or /*. TypeScript also recognizes /// and
        // directives on the last line of a block comment. Reject them anywhere in a block.
        let compiler_whitespace = |c: char| c.is_whitespace() || c == '\u{feff}';
        let compiler_directive = comment
            .text
            .split(['\n', '\r', '\u{2028}', '\u{2029}'])
            .map(|line| {
                let line = if line_comment {
                    line.strip_prefix('/').unwrap_or(line)
                } else {
                    line.trim_start_matches(compiler_whitespace)
                        .trim_start_matches(['/', '*'])
                };
                line.trim_start_matches(compiler_whitespace)
            })
            .find(|directive| {
                directive.starts_with("@ts-ignore")
                    || directive.starts_with("@ts-expect-error")
                    // TypeScript's file-wide pragmas are case-insensitive.
                    || directive
                        .get(.."@ts-nocheck".len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("@ts-nocheck"))
            });
        if lint_directive || compiler_directive.is_some() {
            let allowed = [
                "oxlint-disable-next-line ",
                "oxlint-disable-line ",
                "eslint-disable-next-line ",
                "eslint-disable-line ",
            ]
            .iter()
            .find_map(|prefix| directive.strip_prefix(prefix))
            .and_then(|rest| rest.split_once(" -- "))
            .is_some_and(|(rules, reason)| {
                !reason.trim().is_empty()
                    && rules.split(',').all(|rule| {
                        let rule = rule.trim();
                        rule.contains('/')
                            && rule
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-'))
                    })
            }) || compiler_directive
                .and_then(|directive| directive.strip_prefix("@ts-expect-error -- "))
                .is_some_and(|reason| !reason.trim().is_empty());
            if !allowed || !line_comment {
                report.diagnostics.push(format!("{}: only explained line-level suppressions (`// oxlint-disable-next-line plugin/rule -- reason` or `// @ts-expect-error -- reason`, also `/// @ts-expect-error -- reason`) are allowed; @ts-ignore, @ts-nocheck, and block-comment suppressions are forbidden", path.display()));
            }
        }
    }
    let mut analysis = Analysis {
        text: parsed.text_info_lazy(),
        unresolved: parsed.unresolved_context(),
        sdk: HashSet::new(),
        namespaces: HashSet::new(),
        data_aliases: HashSet::new(),
        contexts: Vec::new(),
        entering_load: false,
        data_calls: Vec::new(),
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
                                    .map_or(specifier.local.sym.as_ref() == "yinz", |name| {
                                        name.atom().as_ref() == "yinz"
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
    let data_calls = std::mem::take(&mut analysis.data_calls);
    drop(analysis);
    if report
        .outline
        .iter()
        .any(|entry| entry.kind == OutlineKind::Tool)
        && !report
            .permissions
            .contains(&PermissionType::MessageAndLaunchOtherPlugins)
    {
        report.warnings.push("Hocus tool registration requires MessageAndLaunchOtherPlugins; request it and wait for approval before calling yinz.hocus.tools".into());
    }
    data_store_findings(&mut report, &data_calls);
    report.outline.sort_by_key(|entry| entry.line);
    let result = loader::transpile_native(path, source, true);
    if let Err(error) = result {
        report.diagnostics.push(format!("{error:#}"));
    }
    report
}

/// Common authoring mistakes, detected here
/// from the AST with bindings resolved: a `shared:` namespace without the permission it needs,
/// a reserved column declared, the store used without `PluginData`, and a data call made in
/// `load` before the permission answer can have arrived. Each names the fix; none refuses.
fn data_store_findings(report: &mut PluginCheckReport, data_calls: &[DataCall]) {
    let asked = |permission: PermissionType| report.permissions.contains(&permission);
    if let Some(call) = data_calls.iter().find(|call| call.during_load) {
        report.warnings.push(format!(
            "line {}: an uncached first load must await yinz.waitForPermissions([...]) before \
             a data call; cached grants take effect synchronously, but cannot make this first \
             load safe",
            call.line
        ));
    }
    let mut said_shared = false;
    for call in data_calls {
        for namespace in &call.namespaces {
            if let Err(error) = validate_namespace(Some(namespace)) {
                report.warnings.push(format!("line {}: {error}", call.line));
            }
            // The only `shared:` in a file that is one: the same word in a comment, a tool
            // description or a string elsewhere is not a namespace.
            if namespace.starts_with(SHARED_NAMESPACE_PREFIX)
                && !asked(PermissionType::SharedPluginData)
                && !said_shared
            {
                said_shared = true;
                report.warnings.push(format!(
                    "line {}: a shared: namespace needs SharedPluginData as well as PluginData, \
                     in every plugin that touches it - the reader too",
                    call.line
                ));
            }
        }
        for attribute in &call.attributes {
            if RESERVED_ATTR_NAMES.contains(&attribute.as_str()) {
                report.warnings.push(format!(
                    "line {}: {attribute:?} is on every row already and cannot be declared - a \
                     define that names it fails whole. Drop the attribute; the column is there",
                    call.line
                ));
            }
        }
    }
    if !data_calls.is_empty() && !asked(PermissionType::PluginData) {
        report.warnings.push(
            "the data store needs yinz.requestPermission(['PluginData']), which this source \
             does not ask for"
                .into(),
        );
    }
}

/// One use of the data store, with what the rules above need from its arguments.
struct DataCall {
    line: usize,
    /// Every `namespace` named in its arguments, and `catalog`'s positional one.
    namespaces: Vec<String>,
    /// The attribute names a `define` declares.
    attributes: Vec<String>,
    /// Made in `load` before any `await` of the permission answer, or at module top level.
    during_load: bool,
}

/// Which function the walk is inside, innermost last.
enum Ctx {
    Load { awaited_permissions: bool },
    Other,
}

struct Analysis<'a> {
    text: &'a SourceTextInfo,
    unresolved: SyntaxContext,
    sdk: HashSet<Id>,
    namespaces: HashSet<Id>,
    /// `const D = yinz.data` - the examples' spelling.
    data_aliases: HashSet<Id>,
    contexts: Vec<Ctx>,
    /// Set by the property whose key is `load`, consumed by the function it holds.
    entering_load: bool,
    data_calls: Vec<DataCall>,
    report: &'a mut PluginCheckReport,
}

/// Every string literal under a `namespace` key in an object literal, at any depth.
fn namespaces_in(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Object(object) => {
            for prop in &object.props {
                if let PropOrSpread::Prop(prop) = prop {
                    if let Prop::KeyValue(prop) = &**prop {
                        if key(&prop.key).as_deref() == Some("namespace") {
                            if let Some(namespace) = literal(&prop.value) {
                                out.push(namespace);
                            }
                        }
                        namespaces_in(&prop.value, out);
                    }
                }
            }
        },
        Expr::Array(array) => {
            for item in array.elems.iter().flatten() {
                namespaces_in(&item.expr, out);
            }
        },
        Expr::Paren(paren) => namespaces_in(&paren.expr, out),
        Expr::TsAs(cast) => namespaces_in(&cast.expr, out),
        Expr::TsSatisfies(cast) => namespaces_in(&cast.expr, out),
        Expr::TsConstAssertion(cast) => namespaces_in(&cast.expr, out),
        Expr::TsNonNull(cast) => namespaces_in(&cast.expr, out),
        _ => {},
    }
}

/// The non-computed keys of an object literal.
fn object_keys(expr: &Expr) -> Vec<String> {
    let Expr::Object(object) = expr else {
        return Vec::new();
    };
    object
        .props
        .iter()
        .filter_map(|prop| match prop {
            PropOrSpread::Prop(prop) => match &**prop {
                Prop::KeyValue(prop) => key(&prop.key),
                Prop::Shorthand(id) => Some(id.sym.to_string()),
                Prop::Method(prop) => key(&prop.key),
                _ => None,
            },
            PropOrSpread::Spread(_) => None,
        })
        .collect()
}

const DATA_METHODS: &[&str] = &[
    "define", "catalog", "fetch", "count", "apply", "watch", "put", "update", "delete",
];

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
    /// Whether `expr` is the SDK's `data` object, by member access or through an alias.
    fn data_object(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(id) => self.data_aliases.contains(&id.to_id()),
            Expr::Member(member) => {
                self.sdk_object(&member.obj) && property(&member.prop).as_deref() == Some("data")
            },
            _ => false,
        }
    }
    fn enter(&mut self) {
        let context = if std::mem::take(&mut self.entering_load) {
            Ctx::Load {
                awaited_permissions: false,
            }
        } else {
            Ctx::Other
        };
        self.contexts.push(context);
    }
    fn leave(&mut self) {
        self.contexts.pop();
    }
    fn record_data_call(&mut self, call: &CallExpr, method: &str) {
        let mut namespaces = Vec::new();
        for arg in call.args.iter().filter(|arg| arg.spread.is_none()) {
            namespaces_in(&arg.expr, &mut namespaces);
        }
        if method == "catalog" {
            if let Some(namespace) = call.args.first().and_then(|arg| literal(&arg.expr)) {
                namespaces.push(namespace);
            }
        }
        let attributes = match method {
            "define" => call
                .args
                .get(1)
                .filter(|arg| arg.spread.is_none())
                .map(|arg| object_keys(&arg.expr))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        // Module top level runs before `load` and before any answer can arrive either.
        let during_load = match self.contexts.last() {
            None => true,
            Some(Ctx::Load {
                awaited_permissions,
            }) => !awaited_permissions,
            Some(Ctx::Other) => false,
        };
        self.data_calls.push(DataCall {
            line: self.line(call.span),
            namespaces,
            attributes,
            during_load,
        });
    }
    fn sdk_object(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(id) => {
                self.sdk.contains(&id.to_id()) || (id.sym == "yinz" && id.ctxt == self.unresolved)
            },
            Expr::Member(member) => match &*member.obj {
                Expr::Ident(id) => {
                    (self.namespaces.contains(&id.to_id())
                        && matches!(property(&member.prop).as_deref(), Some("yinz" | "default")))
                        || (id.sym == "globalThis"
                            && id.ctxt == self.unresolved
                            && property(&member.prop).as_deref() == Some("yinz"))
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
                Err(_) => {
                    let suggestion = yinz_plugin_api::authoring::nearest_permission_type(&name)
                        .map(|nearest| format!(" - did you mean \"{nearest}\"?"))
                        .unwrap_or_else(|| ".".to_owned());
                    self.report.warnings.push(format!(
                        "line {}: unknown permission {name:?} grants nothing{suggestion} The \
                         names are the Permission type in yinz:plugin",
                        self.line(span)
                    ))
                },
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
        let name = key(&method.key);
        if let Some(name) = &name {
            self.entry(
                method.function.span,
                OutlineKind::Function,
                format!("{name}(...)"),
            );
        }
        self.entering_load = name.as_deref() == Some("load");
        method.visit_children_with(self);
        self.entering_load = false;
    }
    fn visit_key_value_prop(&mut self, prop: &KeyValueProp) {
        self.entering_load = key(&prop.key).as_deref() == Some("load")
            && matches!(&*prop.value, Expr::Arrow(_) | Expr::Fn(_));
        prop.visit_children_with(self);
        self.entering_load = false;
    }
    fn visit_function(&mut self, function: &Function) {
        self.enter();
        function.visit_children_with(self);
        self.leave();
    }
    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.enter();
        arrow.visit_children_with(self);
        self.leave();
    }
    fn visit_await_expr(&mut self, expr: &AwaitExpr) {
        // `await yinz.waitForPermissions([...])`, or the raw event wait, is the moment the
        // answer is in; a data call after it in the same `load` is safe.
        if let Expr::Call(call) = &*expr.arg {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = &**callee {
                    let answered = self.sdk_object(&member.obj)
                        && match property(&member.prop).as_deref() {
                            Some("waitForPermissions") => true,
                            Some("waitForEvent") => {
                                call.args
                                    .first()
                                    .and_then(|arg| literal(&arg.expr))
                                    .as_deref()
                                    == Some("PermissionRequestResult")
                            },
                            _ => false,
                        };
                    if answered {
                        if let Some(Ctx::Load {
                            awaited_permissions,
                        }) = self.contexts.last_mut()
                        {
                            *awaited_permissions = true;
                        }
                    }
                }
            }
        }
        expr.visit_children_with(self);
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
        if let (Pat::Ident(id), Some(init)) = (&declaration.name, declaration.init.as_deref()) {
            if self.data_object(init) {
                self.data_aliases.insert(id.to_id());
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
                // yinz.host.requestPluginPermissions([...]) is the raw command's sugar.
                if property(&member.prop).as_deref() == Some("requestPluginPermissions") {
                    if let Expr::Member(host) = &*member.obj {
                        if self.sdk_object(&host.obj)
                            && property(&host.prop).as_deref() == Some("host")
                        {
                            let argument = call
                                .args
                                .first()
                                .filter(|arg| arg.spread.is_none())
                                .map(|arg| &*arg.expr);
                            self.permissions(call.span, argument);
                        }
                    }
                }
                // yinz.data.<method>(...), or through `const D = yinz.data`.
                if let Some(method) = property(&member.prop)
                    .filter(|method| DATA_METHODS.contains(&method.as_str()))
                    .filter(|_| self.data_object(&member.obj))
                {
                    self.record_data_call(call, &method);
                }
                // yinz.host.data<Command>(...) is the raw command's sugar.
                if let Expr::Member(host) = &*member.obj {
                    if self.sdk_object(&host.obj)
                        && property(&host.prop).as_deref() == Some("host")
                        && property(&member.prop).is_some_and(|name| name.starts_with("data"))
                    {
                        self.record_data_call(call, "raw");
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
                            Some(expr)
                                if literal(expr).is_some_and(|name| name.starts_with("Data")) =>
                            {
                                self.record_data_call(call, "raw")
                            },
                            Some(Expr::Object(object)) => {
                                // A command object is exactly one own data field.
                                if let [PropOrSpread::Prop(prop)] = object.props.as_slice() {
                                    if let Prop::KeyValue(prop) = &**prop {
                                        match key(&prop.key) {
                                            Some(name) if name == "RequestPluginPermissions" => {
                                                self.permissions(call.span, Some(&prop.value));
                                            },
                                            Some(name) if name.starts_with("Data") => {
                                                self.record_data_call(call, "raw");
                                            },
                                            _ => {},
                                        }
                                    }
                                }
                            },
                            _ => {},
                        },
                        Some("on" | "onEvent" | "waitForEvent") => {
                            if let Some(name) = argument(0).and_then(literal) {
                                self.entry(call.span, OutlineKind::Handler, format!("on {name}"));
                            }
                        },
                        Some("registerDefaultKeybindings") => {
                            self.entry(
                                call.span,
                                OutlineKind::Handler,
                                "default keybindings".into(),
                            );
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
