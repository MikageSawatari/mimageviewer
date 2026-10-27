use super::*;

/// Paths are repository-relative, slash-separated and lexically normalized. No filesystem
/// access or host cfg evaluation: all possible module routes contribute to reachability.
#[derive(Default)]
pub(super) struct ModuleTree {
    pub test_only: BTreeSet<String>,
    pub missing: BTreeSet<String>,
    // The disk adapter must probe both alternatives, even if one is already in
    // the source map, before accepting the resolver's final classification.
    pub module_candidates: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceRoute {
    file: String,
    module_dir: String,
    path_dir: String,
    test: bool,
}

fn normalize(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|part| *part != "..") => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn join(directory: &str, path: &str) -> String {
    if path.starts_with(['/', '\\']) || path.as_bytes().get(1) == Some(&b':') {
        // Preserve an absolute path as an unsupported edge, never disguise it as
        // a repository-relative alias of a source that may be test-only.
        return path.replace('\\', "/");
    }
    normalize(&format!("{directory}/{path}"))
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn file_route(file: String, test: bool, root: bool) -> SourceRoute {
    let path_dir = parent(&file).to_owned();
    let module_dir = if root || file.ends_with("/mod.rs") {
        path_dir.clone()
    } else {
        file.trim_end_matches(".rs").to_owned()
    };
    SourceRoute {
        file,
        module_dir,
        path_dir,
        test,
    }
}

// Target kind affects only manifest/filesystem layout, never A6's test gating.
// All discovered paths enter the module resolver through the same non-test route.
struct TargetLayout {
    key: &'static str,
    auto: &'static str,
    singleton: bool,
    default_file: Option<&'static str>,
    directory: Option<&'static str>,
}

const TARGET_LAYOUTS: &[TargetLayout] = &[
    TargetLayout {
        key: "lib",
        auto: "autolib",
        singleton: true,
        default_file: Some("src/lib.rs"),
        directory: None,
    },
    TargetLayout {
        key: "bin",
        auto: "autobins",
        singleton: false,
        default_file: Some("src/main.rs"),
        directory: Some("src/bin"),
    },
    TargetLayout {
        key: "example",
        auto: "autoexamples",
        singleton: false,
        default_file: None,
        directory: Some("examples"),
    },
    TargetLayout {
        key: "test",
        auto: "autotests",
        singleton: false,
        default_file: None,
        directory: Some("tests"),
    },
    TargetLayout {
        key: "bench",
        auto: "autobenches",
        singleton: false,
        default_file: None,
        directory: Some("benches"),
    },
];

/// Sources include a complete inventory of the conventional target directories and
/// package-root .rs files. Extra explicit paths are subsequently read by the loader.
/// Test/bench targets and feature-selected targets are deliberately non-test roots:
/// over-approximating Cargo's target set can only make the A6 exemption stricter.
pub(super) fn crate_roots(
    manifest: &str,
    sources: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let manifest = manifest
        .parse::<toml::Table>()
        .map_err(|error| format!("cannot parse Cargo.toml: {error}"))?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or("unsupported Cargo.toml: expected a package table")?;
    for (key, value) in package {
        if key.starts_with("auto") {
            if !TARGET_LAYOUTS.iter().any(|layout| layout.auto == key) {
                return Err(format!("unsupported auto-discovery setting: {key}"));
            }
            if !value.is_bool() {
                return Err(format!(
                    "unsupported auto-discovery setting {key}: expected a boolean"
                ));
            }
        }
    }

    let mut roots = BTreeSet::new();
    for layout in TARGET_LAYOUTS {
        let mut declarations = Vec::new();
        if let Some(value) = manifest.get(layout.key) {
            if layout.singleton {
                declarations.push(value);
            } else {
                declarations.extend(value.as_array().ok_or_else(|| {
                    format!(
                        "unsupported target {}: expected an array of tables",
                        layout.key
                    )
                })?);
            }
        }
        for declaration in &declarations {
            let table = declaration
                .as_table()
                .ok_or_else(|| format!("unsupported target {}: expected a table", layout.key))?;
            validate_target_fields(layout.key, table)?;
            let path = table
                .get("path")
                .and_then(toml::Value::as_str)
                .or_else(|| layout.singleton.then_some(layout.default_file).flatten())
                .ok_or_else(|| format!("target {} requires an explicit string path", layout.key))?;
            if path.is_empty() {
                return Err(format!("unsupported target {}: empty path", layout.key));
            }
            roots.insert(join("", path));
        }
        // An explicit singleton replaces its inferred default. For array targets
        // retain all auto candidates, even when a same-name declaration overrides
        // one in Cargo; the deliberate superset cannot hide a production route.
        let automatic = package
            .get(layout.auto)
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if automatic && !(layout.singleton && !declarations.is_empty()) {
            if let Some(path) = layout.default_file {
                add_existing_root(sources, path, &mut roots)?;
            }
            if let Some(directory) = layout.directory {
                let prefix = format!("{directory}/");
                let folded = prefix.to_uppercase();
                for path in sources.keys() {
                    if path.to_uppercase().starts_with(&folded) {
                        let relative = path
                            .split('/')
                            .skip(prefix.matches('/').count())
                            .collect::<Vec<_>>();
                        let candidate = match relative.as_slice() {
                            [file]
                                if file.rsplit_once('.').is_some_and(|(_, extension)| {
                                    extension.to_uppercase() == "RS"
                                }) =>
                            {
                                Some(format!("{prefix}{}.rs", file.rsplit_once('.').unwrap().0))
                            }
                            [name, file] if file.to_uppercase() == "MAIN.RS" => {
                                Some(format!("{prefix}{name}/main.rs"))
                            }
                            _ => None,
                        };
                        if let Some(candidate) = candidate {
                            add_existing_root(sources, &candidate, &mut roots)?;
                        }
                    }
                }
            }
        }
    }
    match package.get("build") {
        Some(toml::Value::Boolean(false)) => {}
        None | Some(toml::Value::Boolean(true)) => {
            add_existing_root(sources, "build.rs", &mut roots)?
        }
        Some(toml::Value::String(path)) if !path.is_empty() => {
            roots.insert(join("", path));
        }
        _ => return Err("unsupported package.build: expected a path string or boolean".into()),
    }
    Ok(roots.into_iter().collect())
}

fn add_existing_root(
    sources: &BTreeMap<String, String>,
    path: &str,
    roots: &mut BTreeSet<String>,
) -> Result<(), String> {
    check_path_case(sources, path)?;
    if sources.contains_key(path) {
        roots.insert(path.to_owned());
    }
    Ok(())
}

fn validate_target_fields(kind: &str, target: &toml::Table) -> Result<(), String> {
    for (key, value) in target {
        let valid = match key.as_str() {
            "name" | "path" | "edition" => value.is_str(),
            "test" | "bench" | "doc" | "doctest" | "harness" | "proc-macro" | "plugin" => {
                value.is_bool()
            }
            "crate-type" | "required-features" => value
                .as_array()
                .is_some_and(|values| values.iter().all(toml::Value::is_str)),
            _ => return Err(format!("unsupported target {kind} field: {key}")),
        };
        if !valid {
            return Err(format!("unsupported target {kind} field type: {key}"));
        }
    }
    Ok(())
}

pub(super) fn check_path_case(
    sources: &BTreeMap<String, String>,
    path: &str,
) -> Result<(), String> {
    let folded = path.to_uppercase();
    if let Some(existing) = sources
        .keys()
        .find(|existing| existing.as_str() != path && existing.to_uppercase() == folded)
    {
        return Err(format!(
            "module path case differs from file on disk: {path} (actual: {existing})"
        ));
    }
    Ok(())
}

pub(super) fn resolve(
    sources: &BTreeMap<String, String>,
    roots: &[String],
) -> Result<ModuleTree, String> {
    let mut pending: Vec<_> = roots
        .iter()
        .map(|root| file_route(root.clone(), false, true))
        .collect();
    let mut visited = BTreeSet::new();
    let mut reachability = BTreeMap::<String, bool>::new();
    let mut missing = BTreeSet::new();
    let mut module_candidates = BTreeSet::new();
    while let Some(mut route) = pending.pop() {
        check_path_case(sources, &route.file)?;
        if !visited.insert(route.clone()) {
            continue;
        }
        let Some(source) = sources.get(&route.file) else {
            missing.insert(route.file);
            continue;
        };
        let file = syn::parse_file(source).map_err(|error| {
            format!("cannot parse {} for module resolution: {error}", route.file)
        })?;
        route.test |= file.attrs.iter().any(cfg_implies_test_attribute);
        reachability
            .entry(route.file.clone())
            .and_modify(|test_only| *test_only &= route.test)
            .or_insert(route.test);
        let mut visitor = ModuleVisitor {
            sources,
            route: &route,
            module_dir: route.module_dir.clone(),
            path_dir: route.path_dir.clone(),
            cfg_test_depth: usize::from(route.test),
            pending: &mut pending,
            module_candidates: &mut module_candidates,
            error: None,
        };
        visitor.visit_file(&file);
        if let Some(error) = visitor.error {
            return Err(error);
        }
    }
    Ok(ModuleTree {
        test_only: reachability
            .into_iter()
            .filter_map(|(file, test)| test.then_some(file))
            .collect(),
        missing,
        module_candidates,
    })
}

struct ModuleVisitor<'a> {
    sources: &'a BTreeMap<String, String>,
    route: &'a SourceRoute,
    module_dir: String,
    path_dir: String,
    cfg_test_depth: usize,
    pending: &'a mut Vec<SourceRoute>,
    module_candidates: &'a mut BTreeSet<String>,
    error: Option<String>,
}

impl<'ast> Visit<'ast> for ModuleVisitor<'_> {
    visit_test_scopes!();

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        fn contains_path(meta: &syn::Meta) -> bool {
            meta.path().is_ident("path")
                || matches!(meta, syn::Meta::List(list) if parse_nested_meta(list).is_some_and(|nested| nested.iter().any(contains_path)))
        }
        if node.attrs.iter().any(|attribute| {
            attribute.path().is_ident("cfg_attr") && contains_path(&attribute.meta)
        }) {
            self.error = Some(format!(
                "conditional module path is unsupported in {}: {}",
                self.route.file, node.ident
            ));
            return;
        }
        let explicit = node.attrs.iter().find_map(|attribute| {
            if attribute.path().is_ident("path")
                && let syn::Meta::NameValue(value) = &attribute.meta
                && let Expr::Lit(value) = &value.value
                && let syn::Lit::Str(value) = &value.lit
            {
                Some(join(&self.path_dir, &value.value()))
            } else {
                None
            }
        });
        if node.content.is_some() {
            let directory =
                explicit.unwrap_or_else(|| join(&self.module_dir, &node.ident.to_string()));
            let saved_module = std::mem::replace(&mut self.module_dir, directory.clone());
            let saved_path = std::mem::replace(&mut self.path_dir, directory);
            visit::visit_item_mod(self, node);
            self.module_dir = saved_module;
            self.path_dir = saved_path;
        } else {
            let explicit_path = explicit.is_some();
            let file = explicit.unwrap_or_else(|| {
                let stem = join(&self.module_dir, &node.ident.to_string());
                let flat = format!("{stem}.rs");
                let nested = format!("{stem}/mod.rs");
                self.module_candidates
                    .extend([flat.clone(), nested.clone()]);
                for path in [&flat, &nested] {
                    if let Err(error) = check_path_case(self.sources, path) {
                        self.error = Some(error);
                    }
                }
                if self.sources.contains_key(&flat) && self.sources.contains_key(&nested) {
                    self.error = Some(format!(
                        "ambiguous module {}: {flat} and {nested}",
                        node.ident
                    ));
                }
                if self.sources.contains_key(&nested) {
                    nested
                } else {
                    flat
                }
            });
            // An explicit #[path] owns its directory; rustc does not append the
            // filename stem when resolving that module's children.
            self.pending
                .push(file_route(file, self.cfg_test_depth > 0, explicit_path));
        }
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let segments = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let is_include = segments == ["include"]
            || segments == ["std", "include"]
            || segments == ["core", "include"];
        if is_include
            && let Ok(literals) = node.parse_body_with(
                syn::punctuated::Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated,
            )
            && literals.len() == 1
        {
            let literal = &literals[0];
            if !literal.value().ends_with(".rs") {
                self.error = Some(format!(
                    "non-.rs literal include is unsupported in {}: {}",
                    self.route.file,
                    literal.value()
                ));
                return;
            }
            // include! is relative to the source file, even inside inline modules.
            let file = join(parent(&self.route.file), &literal.value());
            self.pending
                .push(file_route(file, self.cfg_test_depth > 0, true));
        }
        // Generated include!(concat!(env!("OUT_DIR"), ...)) is intentionally opaque.
    }
}
