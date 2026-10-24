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

/// The repository uses explicit bin paths and automatic/default library discovery.
/// TOML syntax is handled by the parser; unsupported target shapes/settings fail
/// explicitly instead of silently omitting a possible production crate root.
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
    if package.get("autobins").and_then(toml::Value::as_bool) != Some(false) {
        return Err("unsupported auto-discovery: require explicit autobins = false".into());
    }
    if package
        .get("autolib")
        .is_some_and(|value| value.as_bool() != Some(true))
    {
        return Err("unsupported auto-discovery: require autolib = true or its default".into());
    }
    for key in package.keys() {
        if key.starts_with("auto") && !matches!(key.as_str(), "autobins" | "autolib") {
            return Err(format!("unsupported auto-discovery setting: {key}"));
        }
    }

    let mut roots = BTreeSet::new();
    let library = manifest
        .get("lib")
        .map(|value| {
            value
                .as_table()
                .ok_or("unsupported [lib]: expected a table")
        })
        .transpose()?;
    if let Some(path) = library.and_then(|library| library.get("path")) {
        let path = path
            .as_str()
            .ok_or("unsupported [lib] path: expected a string")?;
        roots.insert(join("", path));
    } else {
        check_path_case(sources, "src/lib.rs")?;
        if sources.contains_key("src/lib.rs") {
            roots.insert("src/lib.rs".into());
        }
    }
    if let Some(binaries) = manifest.get("bin") {
        let binaries = binaries
            .as_array()
            .ok_or("unsupported [[bin]]: expected an array of tables")?;
        for binary in binaries {
            let binary = binary
                .as_table()
                .ok_or("unsupported [[bin]]: expected a table")?;
            let path = binary
                .get("path")
                .and_then(toml::Value::as_str)
                .ok_or("[[bin]] requires an explicit string path")?;
            roots.insert(join("", path));
        }
    }
    Ok(roots.into_iter().collect())
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
