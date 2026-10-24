use super::*;

/// Paths are repository-relative, slash-separated and lexically normalized. No filesystem
/// access or host cfg evaluation: all possible module routes contribute to reachability.
#[derive(Default)]
pub(super) struct ModuleTree {
    pub test_only: BTreeSet<String>,
    pub missing: BTreeSet<String>,
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

/// The repository declares explicit bin paths (autobins=false). Read those declarations
/// without adding a TOML dependency to this source-only tool. Reject unsupported path
/// syntax/missing paths instead of silently omitting a possible production crate root.
pub(super) fn crate_roots(manifest: &str) -> Result<Vec<String>, String> {
    let mut roots = BTreeSet::new();
    let mut in_bin = false;
    let mut bin_path = None;
    for line in manifest.lines().chain(std::iter::once("[end]")) {
        let line = line.trim();
        if line.starts_with('[') {
            let header = line.split('#').next().unwrap().trim();
            if header.starts_with("[[")
                && !header.chars().all(|c| {
                    c.is_ascii_alphanumeric() || "[]_.-".contains(c) || c.is_ascii_whitespace()
                })
            {
                return Err(format!("unsupported Cargo.toml table header: {header}"));
            }
            if in_bin {
                roots.insert(bin_path.take().ok_or("[[bin]] requires an explicit path")?);
            }
            in_bin = header
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>()
                == "[[bin]]";
        } else if in_bin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "path"
        {
            // syn accepts quoted strings and trailing comments, as used by Cargo.toml.
            let path = syn::parse_str::<syn::LitStr>(value.trim())
                .map_err(|error| format!("unsupported [[bin]] path: {error}"))?;
            if bin_path.replace(join("", &path.value())).is_some() {
                return Err("duplicate [[bin]] path".into());
            }
        }
    }
    Ok(roots.into_iter().collect())
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
    while let Some(mut route) = pending.pop() {
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
    })
}

struct ModuleVisitor<'a> {
    sources: &'a BTreeMap<String, String>,
    route: &'a SourceRoute,
    module_dir: String,
    path_dir: String,
    cfg_test_depth: usize,
    pending: &'a mut Vec<SourceRoute>,
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
