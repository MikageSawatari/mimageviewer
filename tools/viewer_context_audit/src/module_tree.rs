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

/// The repository declares explicit bin paths (autobins=false, autolib=true/default).
/// Read those declarations and the explicit/default library path
/// without adding a TOML dependency to this source-only tool. Reject unsupported path
/// syntax/missing paths instead of silently omitting a possible production crate root.
pub(super) fn crate_roots(
    manifest: &str,
    sources: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut roots = BTreeSet::new();
    let mut section = String::new();
    let mut bin_path = None;
    let mut lib_path = None;
    let mut autobins = None;
    let mut autolib = None;
    for line in manifest.lines().chain(std::iter::once("[end]")) {
        let line = strip_toml_comment(line)?.trim();
        if line.starts_with('[') {
            let header = line;
            if header.starts_with("[[")
                && !header.chars().all(|c| {
                    c.is_ascii_alphanumeric() || "[]_.-".contains(c) || c.is_ascii_whitespace()
                })
            {
                return Err(format!("unsupported Cargo.toml table header: {header}"));
            }
            if section == "[[bin]]" {
                roots.insert(bin_path.take().ok_or("[[bin]] requires an explicit path")?);
            }
            section = header
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>();
            // Quoted keys could disguise package/lib settings from this limited
            // reader. The repository needs quoted keys only under target tables.
            if header.contains(['\'', '"']) && !section.starts_with("[target.") {
                return Err(format!("unsupported Cargo.toml table header: {header}"));
            }
        } else if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if section.is_empty() {
                return Err(format!(
                    "unsupported top-level Cargo.toml assignment: {key}"
                ));
            }
            if matches!(section.as_str(), "[package]" | "[lib]" | "[[bin]]")
                && key.contains(['\'', '"'])
            {
                return Err(format!("unsupported Cargo.toml key in {section}: {key}"));
            }
            match (section.as_str(), key) {
                ("[[bin]]" | "[lib]", "path") => {
                    let path = syn::parse_str::<syn::LitStr>(value)
                        .map_err(|error| format!("unsupported {section} path: {error}"))?;
                    let slot = if section == "[lib]" {
                        &mut lib_path
                    } else {
                        &mut bin_path
                    };
                    if slot.replace(join("", &path.value())).is_some() {
                        return Err(format!("duplicate {section} path"));
                    }
                }
                ("[package]", "autobins") => {
                    if autobins.replace(value).is_some() || value != "false" {
                        return Err(
                            "unsupported auto-discovery: require explicit autobins = false".into(),
                        );
                    }
                }
                ("[package]", "autolib") => {
                    if autolib.replace(value).is_some() || value != "true" {
                        return Err(
                            "unsupported auto-discovery: require autolib = true or its default"
                                .into(),
                        );
                    }
                }
                ("[package]", key) if key.starts_with("auto") => {
                    return Err(format!("unsupported auto-discovery setting: {key}"));
                }
                ("[lib]" | "[[bin]]", key) if key.starts_with("path.") => {
                    return Err(format!("unsupported {section} path key: {key}"));
                }
                _ => {}
            }
        }
    }
    if autobins.is_none() {
        return Err("unsupported auto-discovery: require explicit autobins = false".into());
    }
    if let Some(path) = lib_path {
        roots.insert(path);
    } else {
        check_path_case(sources, "src/lib.rs")?;
        if sources.contains_key("src/lib.rs") {
            roots.insert("src/lib.rs".into());
        }
    }
    Ok(roots.into_iter().collect())
}

/// A small single-line TOML reader: quoted # belongs to the value, an unquoted
/// # starts a comment. Reject multiline strings rather than misreading settings.
fn strip_toml_comment(line: &str) -> Result<&str, String> {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(delimiter) if character == delimiter => quote = None,
            None if character == '#' => return Ok(&line[..index]),
            None if matches!(character, '\'' | '"') => quote = Some(character),
            _ => {}
        }
    }
    if quote.is_some() {
        return Err("unsupported multiline or unterminated Cargo.toml string".into());
    }
    Ok(line)
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
