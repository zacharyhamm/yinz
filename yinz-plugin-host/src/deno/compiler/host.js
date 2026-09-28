// Runs only in the isolated compiler runtime. Never evaluates a plugin buffer.
function yinzCheck({ entry, sdk, testing, strictOptions, diagnostics }) {
  const prefix = "/$yinz/";
  const sdkPath = prefix + "sdk.d.ts";
  const ops = Deno.core.ops;
  const sources = new Map();
  const options = {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    moduleDetection: ts.ModuleDetectionKind.Force,
    lib: ["lib.es2022.d.ts"],
    types: [],
    strict: true,
    allowJs: true,
    checkJs: true,
    allowImportingTsExtensions: true,
    noEmit: true,
    noUncheckedSideEffectImports: true,
    skipLibCheck: false,
  };
  for (const [name, value] of Object.entries(strictOptions)) {
    if (typeof value === "boolean") options[name] = value;
  }
  function readFile(name) {
    if (name === sdkPath) return sdk;
    if (name === prefix + "testing.d.ts" && testing) return testing;
    if (name.startsWith(prefix)) return __yinzLibs[name.slice(prefix.length)];
    return ops.op_yinz_check_read(name);
  }
  const host = {
    getSourceFile(name, languageVersion) {
      if (sources.has(name)) return sources.get(name);
      const text = readFile(name);
      if (text === undefined) return undefined;
      const source = ts.createSourceFile(name, text, languageVersion, true);
      sources.set(name, source);
      if (!name.startsWith(prefix)) {
        for (const ref of source.referencedFiles) {
          ops.op_yinz_check_resolve(ref.fileName, name);
        }
        if (source.typeReferenceDirectives.length) {
          throw new Error(`${name}: package type references are not supported`);
        }
      }
      return source;
    },
    getDefaultLibFileName: () => prefix + "lib.es2022.d.ts",
    getDefaultLibLocation: () => prefix.slice(0, -1),
    getCurrentDirectory: () => "/",
    getCanonicalFileName: name => name,
    useCaseSensitiveFileNames: () => true,
    getNewLine: () => "\n",
    readFile,
    fileExists: name => name === sdkPath ||
      (name.startsWith(prefix) && Object.hasOwn(__yinzLibs, name.slice(prefix.length))),
    directoryExists: name => name === prefix.slice(0, -1),
    getDirectories: () => [],
    writeFile() { throw new Error("offline checker cannot emit files"); },
    resolveModuleNames(names, containingFile) {
      return names.map(name => {
        const resolvedFileName = ops.op_yinz_check_resolve(name, containingFile);
        const extension = resolvedFileName.endsWith(".d.ts") ? ts.Extension.Dts :
          resolvedFileName.endsWith(".mts") ? ts.Extension.Mts :
          resolvedFileName.endsWith(".ts") ? ts.Extension.Ts :
          resolvedFileName.endsWith(".mjs") ? ts.Extension.Mjs : ts.Extension.Js;
        return { resolvedFileName, extension, isExternalLibraryImport: resolvedFileName === sdkPath };
      });
    },
    resolveTypeReferenceDirectives: names => names.map(() => undefined),
  };
  const program = ts.createProgram([entry, sdkPath], options, host);
  for (const [name, source] of sources) {
    if (name.startsWith(prefix)) continue;
    const imports = [];
    const isJavaScript = ts.isSourceFileJS(source);
    function visit(node) {
      if (ts.isStringLiteralLike(node) && (
        ((ts.isImportDeclaration(node.parent) || ts.isExportDeclaration(node.parent)) && node.parent.moduleSpecifier === node) ||
        (ts.isModuleDeclaration(node.parent) && node.parent.name === node) ||
        (ts.isJSDocImportTag(node.parent) && node.parent.moduleSpecifier === node) ||
        (ts.isLiteralTypeNode(node.parent) && ts.isImportTypeNode(node.parent.parent)) ||
        (ts.isCallExpression(node.parent) && node.parent.expression.kind === ts.SyntaxKind.ImportKeyword && node.parent.arguments[0] === node)
      )) {
        const start = node.getStart(source);
        imports.push({start, end: node.end,
          target: ops.op_yinz_check_resolve(node.text, name)});
      }
      if (isJavaScript) {
        for (const jsDoc of node.jsDoc || []) visit(jsDoc);
      }
      ts.forEachChild(node, visit);
    }
    visit(source);
    for (const ref of source.referencedFiles) {
      // Reference spans exclude quotes; native rewrites replace the whole literal.
      imports.push({start: ref.pos - 1, end: ref.end + 1,
        target: ops.op_yinz_check_resolve(ref.fileName, name)});
    }
    imports.sort((a, b) => a.start - b.start);
    ops.op_yinz_check_imports(name, imports);
  }
  const findings = diagnostics ? ts.getPreEmitDiagnostics(program) : program.getSyntacticDiagnostics();
  if (findings.length) {
    throw new Error(ts.formatDiagnostics(findings, host));
  }
}
