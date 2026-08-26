use proc_macro2::Span;
use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fmt::{self, Write as _};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprAssign, ExprCall, ExprField, ExprMethodCall, ExprReference, FieldPat,
    File, ForeignItem, ImplItem, Item, ItemFn, Member, Pat, PatStruct, ReturnType, Signature,
    TraitItem, Type, UseTree, Visibility,
};

const REGISTRY_PATH: &str = "src/app/viewer_context_registry.rs";
const APP_TESTS_PATH: &str = "src/app/tests.rs";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Rule {
    A1,
    A2a,
    A2b,
    A3,
    A4,
    A5,
    A6,
    A7a,
    A7b,
    A7c,
    A7d,
    A7e,
    A7f,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::A1 => "A1",
            Self::A2a => "A2a",
            Self::A2b => "A2b",
            Self::A3 => "A3",
            Self::A4 => "A4",
            Self::A5 => "A5",
            Self::A6 => "A6",
            Self::A7a => "A7(a)",
            Self::A7b => "A7(b)",
            Self::A7c => "A7(c)",
            Self::A7d => "A7(d)",
            Self::A7e => "A7(e)",
            Self::A7f => "A7(f)",
        };
        f.write_str(name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Violation {
    rule: Rule,
    file: String,
    function: String,
    line: usize,
    message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}:{} {}: {}",
            self.rule, self.file, self.line, self.function, self.message
        )
    }
}

#[derive(Clone, Copy)]
struct AllowlistEntry {
    file: &'static str,
    function: &'static str,
    rule: Rule,
    reason: &'static str,
}

#[derive(Clone, Copy)]
struct KnownFindingEntry {
    file: &'static str,
    function: &'static str,
    rule: Rule,
    reason: &'static str,
    tracking: &'static str,
}

const ALLOWLIST_ENTRIES: &[AllowlistEntry] = &[
    AllowlistEntry {
        file: "src/app/smart_folder.rs",
        function: "preserve_smart_folder_session_for_load",
        rule: Rule::A2b,
        reason: "Moves the current grid surface into SmartFolderPreparedGrid, which remains nested under the same context-owned TopLevelGridView; it preserves an authorized smart-folder drill session and never transfers the viewer context to another window or owner.",
    },
    AllowlistEntry {
        file: "src/app.rs",
        function: "start_loading_items_inner",
        rule: Rule::A2b,
        reason: "Consumes same-named fields from prepared loader metadata/results into the mounted projection; the mem::take receivers are metadata/prepared payloads, not App, so this installs a completed load rather than extracting or transferring an existing viewer context.",
    },
    AllowlistEntry {
        file: "src/app.rs",
        function: "remove_items_batch",
        rule: Rule::A2b,
        reason: "Takes, index-shifts, and immediately reassigns per-item maps after batch deletion; every value stays in the same mounted context and the temporary ownership exists only to transform keys in place.",
    },
    AllowlistEntry {
        file: "src/app/snapshot_ops.rs",
        function: "activate_snapshot",
        rule: Rule::A2b,
        reason: "Stashes six per-context grid fields into ViewerContextBundle::snapshot, which is itself a bundle field exchanged by swap_viewer_context_bundle. Every value stays inside the mounted context and is restored by deactivate_snapshot in that same context; nothing is transferred to another window or owner. This was a KNOWN_FINDINGS entry while App::snapshot was App-global.",
    },
];

// A4 excludes cfg(test)-gated API and freezes only the production registry surface below.
// The ownership design named a viewer_context_registry::test_access namespace that was never
// implemented. A6 is therefore deliberately defined against the implementation that exists:
// cfg(test)-gated App methods/functions ending in _for_test, plus their call sites.
const PUBLIC_API_ALLOWLIST: &[&str] = &[
    "item struct pub (crate) struct ViewerContextId () ;",
    "inherent fn    ViewerContextId ::  pub (in crate :: app) fn serial (self) -> u64",
    "inherent fn    ViewerContextId :: # [cfg (not (windows))] pub (in crate :: app) const fn single_context () -> Self",
    "item enum pub (crate) enum ContextResidence { Mounted , AtRest , Building , Retiring , Retired , Unknown , }",
    "item enum pub (in crate :: app) enum ForkPolicy { LiveMediaPark { window_id : u64 } , MaterializedStillOpen , }",
    "item enum pub (in crate :: app) enum BindError { WindowOwnedBy (ViewerContextId) , ContextOwnedBy (u64) , WrongOrigin (Option < ViewerContextId >) , NotBindable (ContextResidence) , }",
    "item struct pub (crate) struct MountError { pub (crate) id : ViewerContextId , pub (crate) residence : ContextResidence }",
    "item enum pub (in crate :: app) enum RetireError { Building , NotAtRest (ContextResidence) , IsMain , }",
    "item struct # [cfg (windows)] pub (in crate :: app) struct ViewerContextRegistry { }",
    "inherent fn # [cfg (windows)]   ViewerContextRegistry ::  pub (in crate :: app) fn new () -> Self",
    "item enum # [cfg (windows)] pub (in crate :: app) enum BuildOutcome { Commit , Abort (& 'static str) , }",
    "item enum # [cfg (windows)] pub (in crate :: app) enum RetireContextError { Mount (MountError) , Retire (RetireError) , }",
    "item struct # [cfg (windows)] pub (in crate :: app) struct ViewerContextBundle { }",
    "item struct # [cfg (windows)] pub (in crate :: app) struct ContextRef < 'a > { }",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn mounted (app : & 'a App) -> Self",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn at_rest (bundle : & 'a ViewerContextBundle) -> Self",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn fullscreen_idx (self) -> Option < usize >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn items (self) -> & 'a [GridItem]",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn fs_cache (self) -> & 'a ItemsGenerationMap < FsCacheEntry >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn viewer_session_last_sync_stamp (self) -> Option < & 'a ViewerSyncStamp >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn viewer_session_detached_window_id (self) -> Option < u64 >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn pdf_password_request (self) -> Option < & 'a PdfPasswordRequest >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn current_folder (self) -> Option < & 'a Path >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn items_generation (self) -> u64",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn video_audio_mode (self) -> Option < usize >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn video_audio_vst (self) -> Option < & 'a VideoAudioVstState >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn vst3_deferred_media_open (self) -> Option < usize >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn fs_lanczos_cache (self) -> & 'a crate :: gpu_lanczos :: GpuLanczosCache",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn selected (self) -> Option < usize >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn bookmark_view_state (self) -> Option < & 'a BookmarkViewState >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn archive_source_override (self) -> Option < & 'a Path >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn music_bookmarks (self) -> & 'a [crate :: video_bookmarks :: VideoBookmarkMeta]",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn music_bookmarks_loaded (self) -> bool",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn normalize_ui_state (self , idx : usize ,) -> Option < crate :: video :: normalize_types :: NormalizeUiState >",
    "inherent fn # [cfg (windows)]  < 'a > ContextRef < 'a > ::  pub (in crate :: app) fn final_ai_pending_job_id (self , key : & FinalAiKey) -> Option < u64 >",
    "item struct # [cfg (windows)] pub (in crate :: app) struct ContextMut < 'a > { }",
    "inherent fn # [cfg (windows)]   ContextMut < '_ > ::  pub (in crate :: app) fn as_ref (& self) -> ContextRef < '_ >",
    "inherent fn # [cfg (windows)]   ContextMut < '_ > ::  pub (in crate :: app) fn clear_normalize_state (& mut self)",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn pause_mounted_background_work_keep_current_frame (& mut self)",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn activate_mounted_as_independent_detached (& mut self , window_id : u64)",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn become_mounted_independent_detached_viewer (& mut self , window_id : u64 , idx : usize ,)",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn swap_viewer_context_bundle (& mut self , bundle : & mut ViewerContextBundle)",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn split_current_context_preserving_main_grid (& mut self ,) -> Box < ViewerContextBundle >",
    "inherent fn    App :: # [cfg (windows)] pub (in crate :: app) fn split_materialized_physical_context_for_detached_scope (& mut self , physical_context : & Path , idx : usize , items_generation : u64 ,) -> Box < ViewerContextBundle >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn viewer_context_main (& self) -> ViewerContextId",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn mounted_viewer_context_id (& self) -> Option < ViewerContextId >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn projected_viewer_context_id (& self) -> ViewerContextId",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn viewer_context_residence (& self , id : ViewerContextId) -> ContextResidence",
    "inherent fn # [cfg (windows)]   App ::  pub (crate) fn locate_window_context (& self , window_id : u64 ,) -> Option < (ViewerContextId , ContextResidence) >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn viewer_context_window (& self , id : ViewerContextId) -> Option < u64 >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn viewer_context_ids (& self) -> Vec < ViewerContextId >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn other_viewer_context_ids (& self) -> Vec < ViewerContextId >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn with_viewer_context_ref < R > (& self , id : ViewerContextId , f : impl FnOnce (ContextRef < '_ >) -> R ,) -> Option < R >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn bind_window (& mut self , id : ViewerContextId , window_id : u64 ,) -> Result < () , BindError >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn unbind_window (& mut self , window_id : u64) -> Option < ViewerContextId >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn reserve_window_binding_for_build (& mut self , window_id : u64)",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn with_viewer_context < R > (& mut self , id : ViewerContextId , f : impl FnOnce (& mut Self) -> R ,) -> Result < R , MountError >",
    "inherent fn # [cfg (windows)]   App ::  pub (crate) fn with_window_viewer_context < R > (& mut self , window_id : u64 , f : impl FnOnce (& mut Self) -> R ,) -> Result < R , MountError >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn build_viewer_context (& mut self , reason : & 'static str , f : impl FnOnce (& mut Self , ViewerContextId) -> BuildOutcome ,) -> Option < ViewerContextId >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn fork_mounted_live_media_context (& mut self , window_id : u64 ,) -> ViewerContextId",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn fork_materialized_still_context (& mut self , physical_context : & Path , idx : usize ,) -> ViewerContextId",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn retire_context < D > (& mut self , id : ViewerContextId , _reason : & 'static str , digest : impl FnOnce (ContextMut < '_ >) -> D ,) -> Result < D , RetireError >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn close_and_retire_context < D > (& mut self , id : ViewerContextId , reason : & 'static str , finish : impl FnOnce (& mut Self) , digest : impl FnOnce (ContextMut < '_ >) -> D ,) -> Result < D , RetireContextError >",
    "inherent fn # [cfg (windows)]   App ::  pub (in crate :: app) fn stash_mounted_and_start_fresh (& mut self , _reason : & 'static str ,) -> ViewerContextId",
];

/// Violations that are real, understood, and deliberately not fixed yet.
///
/// Empty is the healthy state. An entry is printed on every run, never fails the run, and
/// **fails when it stops being detected** -- that last property is what catches a rule
/// regressing or someone quietly deleting the offending code.
///
/// The one entry this list carried, `activate_snapshot`, was resolved by making
/// `snapshot` a `ViewerContextBundle` field. Note the violation itself did **not**
/// disappear: the function still moves six fields with `mem::replace`. What changed is
/// that the destination is now context-owned, so the entry moved to `ALLOWLIST_ENTRIES`
/// rather than being deleted (deleting it alone would have turned a tracked finding into
/// an untracked violation).
const KNOWN_FINDINGS: &[KnownFindingEntry] = &[];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CliOptions {
    use_allowlist: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct CommandOutput {
    exit_code: u8,
    text: String,
}

#[derive(Debug)]
struct KnownFindingMatch {
    violation: Violation,
    reason: String,
    tracking: String,
}

#[derive(Debug)]
struct AuditReport {
    known_findings: Vec<KnownFindingMatch>,
    violations: Vec<Violation>,
}

pub fn run() -> ExitCode {
    let output = match parse_cli_args(std::env::args_os().skip(1)).and_then(|options| {
        find_repository_root()
            .and_then(|root| audit_repository(&root, options.use_allowlist))
            .map(render_report)
    }) {
        Ok(output) => output,
        Err(error) => CommandOutput {
            exit_code: 2,
            text: format!("viewer context audit failed: {error}\n"),
        },
    };
    if !output.text.is_empty() {
        eprint!("{}", output.text);
    }
    ExitCode::from(output.exit_code)
}

fn parse_cli_args<I, S>(args: I) -> Result<CliOptions, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut use_allowlist = true;
    let mut saw_no_allowlist = false;
    for argument in args {
        let argument = argument.as_ref();
        if argument == "--no-allowlist" && !saw_no_allowlist {
            use_allowlist = false;
            saw_no_allowlist = true;
        } else {
            return Err(format!(
                "unknown or duplicate argument {:?}; usage: viewer_context_audit [--no-allowlist]",
                argument
            ));
        }
    }
    Ok(CliOptions { use_allowlist })
}

fn find_repository_root() -> Result<PathBuf, String> {
    let current = std::env::current_dir()
        .map_err(|error| format!("cannot read current directory: {error}"))?;
    for candidate in current.ancestors() {
        if candidate.join(REGISTRY_PATH).is_file() && candidate.join("Cargo.toml").is_file() {
            return Ok(candidate.to_path_buf());
        }
    }
    Err(format!(
        "could not find repository root containing {REGISTRY_PATH} from {}",
        current.display()
    ))
}

fn audit_repository(root: &Path, use_allowlist: bool) -> Result<AuditReport, String> {
    let registry_source = fs::read_to_string(root.join(REGISTRY_PATH))
        .map_err(|error| format!("cannot read {REGISTRY_PATH}: {error}"))?;
    let bundle_fields = extract_bundle_fields(&registry_source)?;

    let mut files = Vec::new();
    collect_rust_files(&root.join("src"), &mut files)?;
    files.sort();

    let mut violations = audit_public_api(&registry_source, PUBLIC_API_ALLOWLIST)?;
    for path in files {
        let relative = relative_path(root, &path)?;
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {relative}: {error}"))?;
        violations.extend(analyze_source(
            &relative,
            &source,
            &bundle_fields,
            relative == REGISTRY_PATH,
        )?);
        violations.extend(analyze_test_api(
            &relative,
            &source,
            relative == APP_TESTS_PATH,
        )?);
    }
    violations.sort_by(|left, right| {
        (&left.file, left.line, left.rule, &left.function).cmp(&(
            &right.file,
            right.line,
            right.rule,
            &right.function,
        ))
    });
    classify_findings(violations, ALLOWLIST_ENTRIES, KNOWN_FINDINGS, use_allowlist)
}

fn collect_rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read directory {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot read directory entry in {}: {error}",
                directory.display()
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            collect_rust_files(&entry.path(), output)?;
        } else if file_type.is_file() && entry.path().extension().is_some_and(|ext| ext == "rs") {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|error| {
            format!(
                "cannot make {} relative to {}: {error}",
                path.display(),
                root.display()
            )
        })
}

fn validate_allowlist(allowlist: &[AllowlistEntry]) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for entry in allowlist {
        if entry.reason.trim().is_empty() {
            return Err(format!(
                "allowlist entry {} / {} / {} has no reason",
                entry.file, entry.function, entry.rule
            ));
        }
        let key = (entry.file, entry.function, entry.rule);
        if !keys.insert(key) {
            return Err(format!(
                "duplicate allowlist entry {} / {} / {}",
                entry.file, entry.function, entry.rule
            ));
        }
    }
    Ok(())
}

fn validate_known_findings(known_findings: &[KnownFindingEntry]) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for entry in known_findings {
        if entry.reason.trim().is_empty() {
            return Err(format!(
                "known finding entry {} / {} / {} has no reason",
                entry.file, entry.function, entry.rule
            ));
        }
        if entry.tracking.trim().is_empty() {
            return Err(format!(
                "known finding entry {} / {} / {} has no tracking reference",
                entry.file, entry.function, entry.rule
            ));
        }
        let key = (entry.file, entry.function, entry.rule);
        if !keys.insert(key) {
            return Err(format!(
                "duplicate known finding entry {} / {} / {}",
                entry.file, entry.function, entry.rule
            ));
        }
    }
    Ok(())
}

fn classify_findings(
    violations: Vec<Violation>,
    allowlist: &[AllowlistEntry],
    known_findings: &[KnownFindingEntry],
    use_allowlist: bool,
) -> Result<AuditReport, String> {
    validate_allowlist(allowlist)?;
    validate_known_findings(known_findings)?;
    for allowlisted in allowlist {
        if known_findings.iter().any(|known| {
            known.file == allowlisted.file
                && known.function == allowlisted.function
                && known.rule == allowlisted.rule
        }) {
            return Err(format!(
                "entry cannot be both allowlisted and known: {} / {} / {}",
                allowlisted.file, allowlisted.function, allowlisted.rule
            ));
        }
    }

    let mut allowlist_used = vec![false; allowlist.len()];
    let mut known_used = vec![false; known_findings.len()];
    let mut matched_known = Vec::new();
    let mut remaining = Vec::new();
    for violation in violations {
        if let Some((index, entry)) = known_findings.iter().enumerate().find(|(_, entry)| {
            entry.file == violation.file
                && entry.function == violation.function
                && entry.rule == violation.rule
        }) {
            known_used[index] = true;
            matched_known.push(KnownFindingMatch {
                violation,
                reason: entry.reason.to_owned(),
                tracking: entry.tracking.to_owned(),
            });
        } else if let Some((index, _)) = allowlist.iter().enumerate().find(|(_, entry)| {
            entry.file == violation.file
                && entry.function == violation.function
                && entry.rule == violation.rule
        }) {
            allowlist_used[index] = true;
            if !use_allowlist {
                remaining.push(violation);
            }
        } else {
            remaining.push(violation);
        }
    }

    if let Some((index, entry)) = known_findings
        .iter()
        .enumerate()
        .find(|(index, _)| !known_used[*index])
    {
        return Err(format!(
            "known finding stopped matching: {} / {} / {} (entry #{index}, tracked: {})",
            entry.file, entry.function, entry.rule, entry.tracking
        ));
    }
    if let Some((index, entry)) = allowlist
        .iter()
        .enumerate()
        .find(|(index, _)| !allowlist_used[*index])
    {
        return Err(format!(
            "stale allowlist entry {} / {} / {} (entry #{index})",
            entry.file, entry.function, entry.rule
        ));
    }
    Ok(AuditReport {
        known_findings: matched_known,
        violations: remaining,
    })
}

fn render_report(report: AuditReport) -> CommandOutput {
    let mut text = String::new();
    for finding in &report.known_findings {
        let violation = &finding.violation;
        writeln!(
            text,
            "KNOWN FINDING {} {}:{} {}",
            violation.rule, violation.file, violation.line, violation.function
        )
        .expect("writing to String cannot fail");
        writeln!(text, "  detail: {}", violation.message).expect("writing to String cannot fail");
        writeln!(text, "  reason: {}", finding.reason).expect("writing to String cannot fail");
        writeln!(text, "  tracked: {}", finding.tracking).expect("writing to String cannot fail");
    }
    for violation in &report.violations {
        writeln!(text, "{violation}").expect("writing to String cannot fail");
    }
    if !report.known_findings.is_empty() || !report.violations.is_empty() {
        if report.violations.is_empty() {
            writeln!(
                text,
                "viewer context audit: rules A1/A2a/A2b/A3/A4/A5/A6/A7 enabled; {} known finding(s); no untracked violations",
                report.known_findings.len()
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                text,
                "viewer context audit: {} known finding(s); {} violation(s)",
                report.known_findings.len(),
                report.violations.len()
            )
            .expect("writing to String cannot fail");
        }
    }
    CommandOutput {
        exit_code: u8::from(!report.violations.is_empty()),
        text,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ApiFingerprint {
    text: String,
    line: usize,
}

fn audit_public_api(source: &str, allowlist: &[&str]) -> Result<Vec<Violation>, String> {
    let actual = extract_public_api_fingerprints(source)?;
    let expected = allowlist
        .iter()
        .map(|fingerprint| (*fingerprint).to_owned())
        .collect::<BTreeSet<_>>();
    if expected.len() != allowlist.len() {
        return Err("A4 public API allowlist contains duplicate fingerprints".into());
    }

    let actual_by_text = actual
        .into_iter()
        .map(|fingerprint| (fingerprint.text.clone(), fingerprint))
        .collect::<BTreeMap<_, _>>();
    let actual_text = actual_by_text.keys().cloned().collect::<BTreeSet<_>>();
    let mut violations = Vec::new();
    for unexpected in actual_text.difference(&expected) {
        let fingerprint = &actual_by_text[unexpected];
        violations.push(Violation {
            rule: Rule::A4,
            file: REGISTRY_PATH.to_owned(),
            function: "<public-api>".into(),
            line: fingerprint.line,
            message: format!("unexpected public API fingerprint: {unexpected}"),
        });
    }
    for missing in expected.difference(&actual_text) {
        violations.push(Violation {
            rule: Rule::A4,
            file: REGISTRY_PATH.to_owned(),
            function: "<public-api>".into(),
            line: 1,
            message: format!("allowlisted public API fingerprint is missing: {missing}"),
        });
    }
    Ok(violations)
}

fn extract_public_api_fingerprints(source: &str) -> Result<Vec<ApiFingerprint>, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("cannot parse {REGISTRY_PATH} for A4: {error}"))?;
    let public_types = file
        .items
        .iter()
        .filter_map(public_type_name)
        .collect::<BTreeSet<_>>();
    let mut fingerprints = Vec::new();

    for item in &file.items {
        if item_attributes(item).iter().any(cfg_implies_test_attribute) {
            continue;
        }
        match item {
            Item::Fn(item) if visibility_is_public(&item.vis) => {
                fingerprints.push(ApiFingerprint {
                    text: format!(
                        "item fn {} {} {}",
                        relevant_attributes(&item.attrs),
                        token_string(&item.vis),
                        token_string(&item.sig)
                    ),
                    line: item.sig.ident.span().start().line,
                })
            }
            Item::Struct(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                match &mut public.fields {
                    syn::Fields::Named(fields) => {
                        fields.named = fields
                            .named
                            .clone()
                            .into_iter()
                            .filter(|field| visibility_is_public(&field.vis))
                            .map(|mut field| {
                                field.attrs.retain(is_api_attribute);
                                field
                            })
                            .collect();
                    }
                    syn::Fields::Unnamed(fields) => {
                        fields.unnamed = fields
                            .unnamed
                            .clone()
                            .into_iter()
                            .filter(|field| visibility_is_public(&field.vis))
                            .map(|mut field| {
                                field.attrs.retain(is_api_attribute);
                                field
                            })
                            .collect();
                    }
                    syn::Fields::Unit => {}
                }
                fingerprints.push(ApiFingerprint {
                    text: format!("item struct {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Enum(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                for variant in &mut public.variants {
                    variant.attrs.retain(is_api_attribute);
                    for field in &mut variant.fields {
                        field.attrs.retain(is_api_attribute);
                    }
                }
                fingerprints.push(ApiFingerprint {
                    text: format!("item enum {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Union(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                public.fields.named = public
                    .fields
                    .named
                    .clone()
                    .into_iter()
                    .filter(|field| visibility_is_public(&field.vis))
                    .map(|mut field| {
                        field.attrs.retain(is_api_attribute);
                        field
                    })
                    .collect();
                fingerprints.push(ApiFingerprint {
                    text: format!("item union {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Type(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item type {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::TraitAlias(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item trait-alias {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Trait(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item trait {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Const(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item const {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Static(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item static {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Use(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item use {}", token_string(&public)),
                    line: item.span().start().line,
                });
            }
            Item::ExternCrate(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item extern-crate {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::Mod(item) if visibility_is_public(&item.vis) => {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item mod {}", token_string(&public)),
                    line: item.ident.span().start().line,
                });
            }
            Item::ForeignMod(item) => {
                for foreign in &item.items {
                    if foreign_item_visibility(foreign).is_some_and(visibility_is_public) {
                        fingerprints.push(ApiFingerprint {
                            text: format!(
                                "foreign {} {}",
                                relevant_attributes(foreign_item_attributes(foreign)),
                                token_string(foreign)
                            ),
                            line: foreign.span().start().line,
                        });
                    }
                }
            }
            Item::Macro(item)
                if item
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("macro_export")) =>
            {
                let mut public = item.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("item macro {}", token_string(&public)),
                    line: item.span().start().line,
                });
            }
            Item::Impl(item) => {
                collect_impl_api(item, &public_types, &mut fingerprints);
            }
            _ => {}
        }
    }
    fingerprints.sort_by(|left, right| left.text.cmp(&right.text));
    Ok(fingerprints)
}

fn public_type_name(item: &Item) -> Option<String> {
    match item {
        Item::Enum(item) if visibility_is_public(&item.vis) => Some(item.ident.to_string()),
        Item::Struct(item) if visibility_is_public(&item.vis) => Some(item.ident.to_string()),
        Item::Trait(item) if visibility_is_public(&item.vis) => Some(item.ident.to_string()),
        Item::Type(item) if visibility_is_public(&item.vis) => Some(item.ident.to_string()),
        Item::Union(item) if visibility_is_public(&item.vis) => Some(item.ident.to_string()),
        _ => None,
    }
}

fn collect_impl_api(
    item: &syn::ItemImpl,
    public_types: &BTreeSet<String>,
    fingerprints: &mut Vec<ApiFingerprint>,
) {
    if item.attrs.iter().any(cfg_implies_test_attribute) {
        return;
    }
    let self_type = token_string(&item.self_ty);
    let impl_header = format!(
        "{} {} {} {}",
        relevant_attributes(&item.attrs),
        item.unsafety.as_ref().map(token_string).unwrap_or_default(),
        token_string(&item.generics),
        self_type
    );
    if let Some((polarity, trait_path, _)) = &item.trait_ {
        if type_mentions_public_name(&item.self_ty, public_types) {
            let associated = item
                .items
                .iter()
                .map(impl_item_api_tokens)
                .collect::<Vec<_>>()
                .join(" ; ");
            fingerprints.push(ApiFingerprint {
                text: format!(
                    "trait impl {impl_header} {} {} {{ {associated} }}",
                    polarity.as_ref().map(token_string).unwrap_or_default(),
                    token_string(trait_path)
                ),
                line: item.impl_token.span.start().line,
            });
        }
        return;
    }

    for associated in &item.items {
        if impl_item_attributes(associated)
            .iter()
            .any(cfg_implies_test_attribute)
        {
            continue;
        }
        match associated {
            ImplItem::Fn(method) if visibility_is_public(&method.vis) => {
                fingerprints.push(ApiFingerprint {
                    text: format!(
                        "inherent fn {impl_header} :: {} {} {}",
                        relevant_attributes(&method.attrs),
                        token_string(&method.vis),
                        token_string(&method.sig)
                    ),
                    line: method.sig.ident.span().start().line,
                });
            }
            ImplItem::Const(value) if visibility_is_public(&value.vis) => {
                let mut public = value.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("inherent const {impl_header} :: {}", token_string(&public)),
                    line: value.ident.span().start().line,
                });
            }
            ImplItem::Type(value) if visibility_is_public(&value.vis) => {
                let mut public = value.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("inherent type {impl_header} :: {}", token_string(&public)),
                    line: value.ident.span().start().line,
                });
            }
            ImplItem::Macro(value)
                if value
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("macro_export")) =>
            {
                let mut public = value.clone();
                public.attrs.retain(is_api_attribute);
                fingerprints.push(ApiFingerprint {
                    text: format!("inherent macro {impl_header} :: {}", token_string(&public)),
                    line: value.span().start().line,
                });
            }
            _ => {}
        }
    }
}

fn impl_item_api_tokens(item: &ImplItem) -> String {
    match item {
        ImplItem::Fn(method) => format!(
            "{} {} {}",
            relevant_attributes(&method.attrs),
            token_string(&method.vis),
            token_string(&method.sig)
        ),
        ImplItem::Const(value) => {
            let mut value = value.clone();
            value.attrs.retain(is_api_attribute);
            token_string(&value)
        }
        ImplItem::Type(value) => {
            let mut value = value.clone();
            value.attrs.retain(is_api_attribute);
            token_string(&value)
        }
        ImplItem::Macro(value) => {
            let mut value = value.clone();
            value.attrs.retain(is_api_attribute);
            token_string(&value)
        }
        ImplItem::Verbatim(tokens) => tokens.to_string(),
        _ => String::new(),
    }
}

fn type_mentions_public_name(ty: &Type, public_types: &BTreeSet<String>) -> bool {
    struct Finder<'a> {
        public_types: &'a BTreeSet<String>,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            if path
                .segments
                .iter()
                .any(|segment| self.public_types.contains(&segment.ident.to_string()))
            {
                self.found = true;
            }
            visit::visit_path(self, path);
        }
    }
    let mut finder = Finder {
        public_types,
        found: false,
    };
    finder.visit_type(ty);
    finder.found
}

fn visibility_is_public(visibility: &Visibility) -> bool {
    !matches!(visibility, Visibility::Inherited)
}

fn foreign_item_visibility(item: &ForeignItem) -> Option<&Visibility> {
    match item {
        ForeignItem::Fn(item) => Some(&item.vis),
        ForeignItem::Static(item) => Some(&item.vis),
        ForeignItem::Type(item) => Some(&item.vis),
        _ => None,
    }
}

fn is_api_attribute(attribute: &Attribute) -> bool {
    attribute.path().is_ident("cfg")
        || attribute.path().is_ident("cfg_attr")
        || attribute.path().is_ident("macro_export")
}

fn relevant_attributes(attributes: &[Attribute]) -> String {
    attributes
        .iter()
        .filter(|attribute| is_api_attribute(attribute))
        .map(token_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_string(tokens: &impl ToTokens) -> String {
    tokens.to_token_stream().to_string()
}

fn extract_bundle_fields(source: &str) -> Result<BTreeSet<String>, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("cannot parse {REGISTRY_PATH}: {error}"))?;
    let mut finder = BundleStructFinder::default();
    finder.visit_file(&file);
    match finder.matches.as_slice() {
        [] => Err("struct ViewerContextBundle was not found; refusing to skip A2".into()),
        [fields] if fields.is_empty() => {
            Err("struct ViewerContextBundle has no named fields; refusing to skip A2".into())
        }
        [fields] => Ok(fields.clone()),
        _ => Err("multiple struct ViewerContextBundle definitions were found".into()),
    }
}

#[derive(Default)]
struct BundleStructFinder {
    matches: Vec<BTreeSet<String>>,
}

impl<'ast> Visit<'ast> for BundleStructFinder {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if node.ident == "ViewerContextBundle" {
            self.matches.push(
                node.fields
                    .iter()
                    .filter_map(|field| field.ident.as_ref())
                    .map(ToString::to_string)
                    .collect(),
            );
        }
        visit::visit_item_struct(self, node);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemOperation {
    Swap,
    Replace,
    Take,
}

impl MemOperation {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "swap" => Some(Self::Swap),
            "replace" => Some(Self::Replace),
            "take" => Some(Self::Take),
            _ => None,
        }
    }
}

#[derive(Default)]
struct ImportNormalizer {
    aliases: HashMap<String, Vec<String>>,
    glob_modules: Vec<Vec<String>>,
}

impl ImportNormalizer {
    fn from_file(file: &File) -> Self {
        let mut collector = ImportCollector::default();
        collector.visit_file(file);
        collector.imports
    }

    fn mem_operation(&self, path: &syn::Path) -> Option<MemOperation> {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if let Some(operation) = canonical_mem_operation(&segments) {
            return Some(operation);
        }
        let (first, rest) = segments.split_first()?;
        if let Some(prefix) = self.aliases.get(first) {
            let mut canonical = prefix.clone();
            canonical.extend_from_slice(rest);
            if let Some(operation) = canonical_mem_operation(&canonical) {
                return Some(operation);
            }
        }
        if rest.is_empty() {
            for module in &self.glob_modules {
                let mut canonical = module.clone();
                canonical.push(first.clone());
                if let Some(operation) = canonical_mem_operation(&canonical) {
                    return Some(operation);
                }
            }
        }
        None
    }
}

fn canonical_mem_operation(segments: &[String]) -> Option<MemOperation> {
    match segments {
        [root, module, operation] if (root == "std" || root == "core") && module == "mem" => {
            MemOperation::from_name(operation)
        }
        _ => None,
    }
}

#[derive(Default)]
struct ImportCollector {
    imports: ImportNormalizer,
}

impl<'ast> Visit<'ast> for ImportCollector {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        collect_use_tree(&node.tree, &mut Vec::new(), &mut self.imports);
        visit::visit_item_use(self, node);
    }
}

fn collect_use_tree(tree: &UseTree, prefix: &mut Vec<String>, imports: &mut ImportNormalizer) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(&path.tree, prefix, imports);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let ident = name.ident.to_string();
            let canonical = if ident == "self" {
                prefix.clone()
            } else {
                let mut path = prefix.clone();
                path.push(ident.clone());
                path
            };
            if let Some(local) = canonical.last() {
                imports.aliases.insert(local.clone(), canonical);
            }
        }
        UseTree::Rename(rename) => {
            let ident = rename.ident.to_string();
            let canonical = if ident == "self" {
                prefix.clone()
            } else {
                let mut path = prefix.clone();
                path.push(ident);
                path
            };
            imports.aliases.insert(rename.rename.to_string(), canonical);
        }
        UseTree::Glob(_) => imports.glob_modules.push(prefix.clone()),
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix, imports);
            }
        }
    }
}

fn analyze_source(
    path: &str,
    source: &str,
    bundle_fields: &BTreeSet<String>,
    is_registry: bool,
) -> Result<Vec<Violation>, String> {
    let file = syn::parse_file(source).map_err(|error| format!("cannot parse {path}: {error}"))?;
    if is_registry {
        return Ok(Vec::new());
    }
    let imports = ImportNormalizer::from_file(&file);
    let mut visitor = AuditVisitor {
        path,
        bundle_fields,
        imports,
        cfg_test_depth: 0,
        functions: Vec::new(),
        violations: Vec::new(),
        field_context: FieldContext::Value,
        mem_call_depth: 0,
    };
    visitor.visit_file(&file);
    visitor.violations.sort_by(|left, right| {
        (left.line, left.rule, &left.function).cmp(&(right.line, right.rule, &right.function))
    });
    Ok(visitor.violations)
}

fn analyze_test_api(
    path: &str,
    source: &str,
    test_module_file: bool,
) -> Result<Vec<Violation>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("cannot parse {path} for A6: {error}"))?;
    let mut visitor = TestApiVisitor {
        path,
        cfg_test_depth: usize::from(test_module_file),
        functions: Vec::new(),
        violations: Vec::new(),
    };
    visitor.visit_file(&file);
    Ok(visitor.violations)
}

struct TestApiVisitor<'a> {
    path: &'a str,
    cfg_test_depth: usize,
    functions: Vec<String>,
    violations: Vec<Violation>,
}

impl TestApiVisitor<'_> {
    fn current_function(&self) -> String {
        self.functions
            .last()
            .cloned()
            .unwrap_or_else(|| "<module>".into())
    }

    fn record(&mut self, span: Span, message: impl Into<String>) {
        self.violations.push(Violation {
            rule: Rule::A6,
            file: self.path.to_owned(),
            function: self.current_function(),
            line: span.start().line,
            message: message.into(),
        });
    }

    fn check_definition(&mut self, signature: &Signature) {
        if self.cfg_test_depth == 0 && signature.ident.to_string().ends_with("_for_test") {
            self.record(
                signature.ident.span(),
                format!(
                    "test-only definition {} is not guarded by cfg(test)",
                    signature.ident
                ),
            );
        }
    }
}

impl<'ast> Visit<'ast> for TestApiVisitor<'_> {
    fn visit_item(&mut self, node: &'ast Item) {
        let is_test = item_attributes(node).iter().any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        let is_test = impl_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_impl_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_trait_item(&mut self, node: &'ast TraitItem) {
        let is_test = trait_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_trait_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_foreign_item(&mut self, node: &'ast ForeignItem) {
        let is_test = foreign_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_foreign_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.check_definition(&node.sig);
        self.functions.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.check_definition(&node.sig);
        self.functions.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.check_definition(&node.sig);
        self.functions.push(node.sig.ident.to_string());
        visit::visit_trait_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_foreign_item_fn(&mut self, node: &'ast syn::ForeignItemFn) {
        self.check_definition(&node.sig);
        visit::visit_foreign_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        let call_is_test = node.attrs.iter().any(cfg_implies_test_attribute);
        if self.cfg_test_depth == 0
            && !call_is_test
            && call_path(node)
                .and_then(|path| path.segments.last())
                .is_some_and(|segment| segment.ident.to_string().ends_with("_for_test"))
        {
            self.record(
                node.span(),
                "test-only function call appears outside cfg(test)",
            );
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let call_is_test = node.attrs.iter().any(cfg_implies_test_attribute);
        if self.cfg_test_depth == 0
            && !call_is_test
            && node.method.to_string().ends_with("_for_test")
        {
            self.record(
                node.method.span(),
                "test-only method call appears outside cfg(test)",
            );
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_block(&mut self, node: &'ast syn::ExprBlock) {
        let is_test = node.attrs.iter().any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_expr_block(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }
}

#[derive(Default)]
struct FunctionFrame {
    name: String,
    line: usize,
    mem_fields: BTreeSet<String>,
    swap_fields: BTreeSet<String>,
}

#[derive(Clone, Copy)]
enum FieldContext {
    Value,
    // syn cannot tell whether a method receiver is consumed or borrowed. Registry methods are the
    // intended API, so method receivers, indexing, and further member access are treated as access;
    // a naked `app.viewer_contexts` in an argument/RHS/return remains a value move (A7e).
    Access,
    AssignmentTarget,
    Borrowed,
}

struct AuditVisitor<'a> {
    path: &'a str,
    bundle_fields: &'a BTreeSet<String>,
    imports: ImportNormalizer,
    cfg_test_depth: usize,
    functions: Vec<FunctionFrame>,
    violations: Vec<Violation>,
    field_context: FieldContext,
    mem_call_depth: usize,
}

impl AuditVisitor<'_> {
    fn current_function(&self) -> String {
        self.functions
            .last()
            .map(|frame| frame.name.clone())
            .unwrap_or_else(|| "<module>".into())
    }

    fn record(&mut self, rule: Rule, span: Span, message: impl Into<String>) {
        self.record_for(rule, self.current_function(), span, message);
    }

    fn record_for(&mut self, rule: Rule, function: String, span: Span, message: impl Into<String>) {
        self.violations.push(Violation {
            rule,
            file: self.path.to_owned(),
            function,
            line: span.start().line,
            message: message.into(),
        });
    }

    fn enter_function(&mut self, signature: &Signature) {
        self.check_registry_return(signature);
        self.functions.push(FunctionFrame {
            name: signature.ident.to_string(),
            line: signature.ident.span().start().line,
            ..FunctionFrame::default()
        });
    }

    fn leave_function(&mut self) {
        let frame = self.functions.pop().expect("balanced function visitor");
        if !frame.swap_fields.is_empty() {
            self.violations.push(Violation {
                rule: Rule::A2a,
                file: self.path.to_owned(),
                function: frame.name.clone(),
                line: frame.line,
                message: format!(
                    "mem::swap touches ViewerContextBundle field(s): {}",
                    join_fields(&frame.swap_fields)
                ),
            });
        }
        if frame.mem_fields.len() >= 3 {
            self.violations.push(Violation {
                rule: Rule::A2b,
                file: self.path.to_owned(),
                function: frame.name,
                line: frame.line,
                message: format!(
                    "{} distinct ViewerContextBundle fields occur in mem::swap/replace/take arguments: {}",
                    frame.mem_fields.len(),
                    join_fields(&frame.mem_fields)
                ),
            });
        }
    }

    fn check_registry_return(&mut self, signature: &Signature) {
        let ReturnType::Type(_, return_type) = &signature.output else {
            return;
        };
        if type_contains_ident(return_type, "ViewerContextRegistry") {
            self.record_for(
                Rule::A7f,
                signature.ident.to_string(),
                return_type.span(),
                "function return type contains ViewerContextRegistry",
            );
        }
    }

    fn with_field_context(&mut self, context: FieldContext, expression: &Expr) {
        let previous = std::mem::replace(&mut self.field_context, context);
        self.visit_expr(expression);
        self.field_context = previous;
    }

    fn type_contains_bundle(&self, ty: &Type) -> bool {
        struct Finder<'a> {
            imports: &'a ImportNormalizer,
            found: bool,
        }

        impl<'ast> Visit<'ast> for Finder<'_> {
            fn visit_path(&mut self, path: &'ast syn::Path) {
                let segments: Vec<String> = path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect();
                let direct = segments
                    .iter()
                    .any(|segment| segment == "ViewerContextBundle");
                let imported = segments.first().is_some_and(|first| {
                    self.imports.aliases.get(first).is_some_and(|canonical| {
                        canonical
                            .iter()
                            .any(|segment| segment == "ViewerContextBundle")
                    })
                });
                if direct || imported {
                    self.found = true;
                }
                visit::visit_path(self, path);
            }
        }

        let mut finder = Finder {
            imports: &self.imports,
            found: false,
        };
        finder.visit_type(ty);
        finder.found
    }
}

impl<'ast> Visit<'ast> for AuditVisitor<'_> {
    fn visit_type(&mut self, node: &'ast Type) {
        if self.type_contains_bundle(node) {
            self.record(
                Rule::A1,
                node.span(),
                "ViewerContextBundle appears in a type position outside the registry module",
            );
            return;
        }
        visit::visit_type(self, node);
    }

    fn visit_ident(&mut self, node: &'ast syn::Ident) {
        if node == "paused_bundle" || node == "active_detached_viewer_context" {
            self.record(
                Rule::A5,
                node.span(),
                format!("forbidden legacy ownership identifier {node} remains"),
            );
        }
        visit::visit_ident(self, node);
    }

    fn visit_member(&mut self, node: &'ast syn::Member) {
        if let syn::Member::Named(ident) = node
            && (ident == "paused_bundle" || ident == "active_detached_viewer_context")
        {
            self.record(
                Rule::A5,
                ident.span(),
                format!("forbidden legacy ownership identifier {ident} remains"),
            );
        }
        visit::visit_member(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        if use_tree_mentions_bundle(&node.tree) {
            self.record(
                Rule::A1,
                node.span(),
                "ViewerContextBundle import, rename, or re-export is forbidden outside the registry module",
            );
        }
        visit::visit_item_use(self, node);
    }

    fn visit_item(&mut self, node: &'ast Item) {
        let is_test = item_attributes(node).iter().any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        let is_test = impl_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_impl_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_trait_item(&mut self, node: &'ast TraitItem) {
        let is_test = trait_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_trait_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_foreign_item(&mut self, node: &'ast ForeignItem) {
        let is_test = foreign_item_attributes(node)
            .iter()
            .any(cfg_implies_test_attribute);
        self.cfg_test_depth += usize::from(is_test);
        visit::visit_foreign_item(self, node);
        self.cfg_test_depth -= usize::from(is_test);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.enter_function(&node.sig);
        visit::visit_item_fn(self, node);
        self.leave_function();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.enter_function(&node.sig);
        visit::visit_impl_item_fn(self, node);
        self.leave_function();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.enter_function(&node.sig);
        visit::visit_trait_item_fn(self, node);
        self.leave_function();
    }

    fn visit_foreign_item_fn(&mut self, node: &'ast syn::ForeignItemFn) {
        self.check_registry_return(&node.sig);
        visit::visit_foreign_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        let operation = call_path(node).and_then(|path| self.imports.mem_operation(path));
        if let Some(operation) = operation {
            let fields = collect_bundle_field_accesses(node, self.bundle_fields);
            if let Some(frame) = self.functions.last_mut() {
                frame.mem_fields.extend(fields.iter().cloned());
                if operation == MemOperation::Swap {
                    frame.swap_fields.extend(fields);
                }
            }
            if node.args.iter().any(expression_is_registry_field_target) {
                self.record(
                    Rule::A7a,
                    node.span(),
                    "mem::swap/replace/take targets App::viewer_contexts",
                );
            }
        }

        if self.cfg_test_depth == 0 && call_path(node).is_some_and(is_bundle_associated_path) {
            self.record(
                Rule::A3,
                node.span(),
                "ViewerContextBundle associated function called outside the registry module",
            );
        }

        if operation.is_some() {
            self.mem_call_depth += 1;
        }
        visit::visit_expr_call(self, node);
        if operation.is_some() {
            self.mem_call_depth -= 1;
        }
    }

    fn visit_expr_assign(&mut self, node: &'ast ExprAssign) {
        if expression_is_registry_field(&node.left) {
            self.record(
                Rule::A7b,
                node.left.span(),
                "App::viewer_contexts is the left-hand side of an assignment",
            );
        }
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        self.with_field_context(FieldContext::AssignmentTarget, &node.left);
        self.with_field_context(FieldContext::Value, &node.right);
    }

    fn visit_expr_reference(&mut self, node: &'ast ExprReference) {
        if node.mutability.is_some()
            && self.mem_call_depth == 0
            && expression_is_registry_field(&node.expr)
        {
            self.record(
                Rule::A7c,
                node.span(),
                "App::viewer_contexts is borrowed mutably outside a mem operation",
            );
        }
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        self.with_field_context(FieldContext::Borrowed, &node.expr);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        self.with_field_context(FieldContext::Access, &node.receiver);
        for argument in &node.args {
            self.with_field_context(FieldContext::Value, argument);
        }
    }

    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(ident) = &node.member
            && (ident == "paused_bundle" || ident == "active_detached_viewer_context")
        {
            self.record(
                Rule::A5,
                ident.span(),
                format!("forbidden legacy ownership identifier {ident} remains"),
            );
        }
        if member_is(&node.member, "viewer_contexts")
            && matches!(self.field_context, FieldContext::Value)
        {
            self.record(
                Rule::A7e,
                node.span(),
                "App::viewer_contexts is used as a value",
            );
        }
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        self.with_field_context(FieldContext::Access, &node.base);
    }

    fn visit_expr_index(&mut self, node: &'ast syn::ExprIndex) {
        for attribute in &node.attrs {
            self.visit_attribute(attribute);
        }
        self.with_field_context(FieldContext::Access, &node.expr);
        self.with_field_context(FieldContext::Value, &node.index);
    }

    fn visit_pat_struct(&mut self, node: &'ast PatStruct) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|part| part.ident == "App")
            && node.fields.iter().any(field_pattern_moves_registry)
        {
            self.record(
                Rule::A7d,
                node.span(),
                "App is destructured by value to extract viewer_contexts",
            );
        }
        visit::visit_pat_struct(self, node);
    }
}

fn use_tree_mentions_bundle(tree: &UseTree) -> bool {
    match tree {
        UseTree::Path(path) => {
            path.ident == "ViewerContextBundle" || use_tree_mentions_bundle(&path.tree)
        }
        UseTree::Name(name) => name.ident == "ViewerContextBundle",
        UseTree::Rename(rename) => rename.ident == "ViewerContextBundle",
        UseTree::Glob(_) => false,
        UseTree::Group(group) => group.items.iter().any(use_tree_mentions_bundle),
    }
}

fn join_fields(fields: &BTreeSet<String>) -> String {
    fields.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn call_path(call: &ExprCall) -> Option<&syn::Path> {
    match call.func.as_ref() {
        Expr::Path(path) => Some(&path.path),
        _ => None,
    }
}

fn is_bundle_associated_path(path: &syn::Path) -> bool {
    path.segments
        .iter()
        .take(path.segments.len().saturating_sub(1))
        .any(|segment| segment.ident == "ViewerContextBundle")
}

fn collect_bundle_field_accesses(
    call: &ExprCall,
    bundle_fields: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut collector = BundleFieldCollector {
        bundle_fields,
        found: BTreeSet::new(),
    };
    for argument in &call.args {
        collector.visit_expr(argument);
    }
    collector.found
}

struct BundleFieldCollector<'a> {
    bundle_fields: &'a BTreeSet<String>,
    found: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for BundleFieldCollector<'_> {
    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(name) = &node.member {
            let name = name.to_string();
            if self.bundle_fields.contains(&name) {
                self.found.insert(name);
            }
        }
        visit::visit_expr_field(self, node);
    }
}

fn expression_is_registry_field_target(expression: &Expr) -> bool {
    match expression {
        Expr::Reference(reference) => expression_is_registry_field(&reference.expr),
        Expr::Paren(paren) => expression_is_registry_field_target(&paren.expr),
        Expr::Group(group) => expression_is_registry_field_target(&group.expr),
        _ => expression_is_registry_field(expression),
    }
}

fn expression_is_registry_field(expression: &Expr) -> bool {
    match expression {
        Expr::Field(field) => member_is(&field.member, "viewer_contexts"),
        Expr::Paren(paren) => expression_is_registry_field(&paren.expr),
        Expr::Group(group) => expression_is_registry_field(&group.expr),
        _ => false,
    }
}

fn member_is(member: &Member, expected: &str) -> bool {
    matches!(member, Member::Named(name) if name == expected)
}

fn field_pattern_moves_registry(field: &FieldPat) -> bool {
    member_is(&field.member, "viewer_contexts") && pattern_moves_value(&field.pat)
}

fn pattern_moves_value(pattern: &Pat) -> bool {
    match pattern {
        Pat::Ident(ident) => ident.by_ref.is_none(),
        // Explicit borrowing is recognizable. Match ergonomics can also borrow an unannotated
        // binding from a reference scrutinee, which a source-only syn visitor cannot infer.
        Pat::Reference(_) | Pat::Wild(_) => false,
        Pat::Paren(paren) => pattern_moves_value(&paren.pat),
        _ => true,
    }
}

fn type_contains_ident(ty: &Type, expected: &str) -> bool {
    struct TypeIdentFinder<'a> {
        expected: &'a str,
        found: bool,
    }

    impl<'ast> Visit<'ast> for TypeIdentFinder<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            if path
                .segments
                .iter()
                .any(|segment| segment.ident == self.expected)
            {
                self.found = true;
            }
            visit::visit_path(self, path);
        }
    }

    let mut finder = TypeIdentFinder {
        expected,
        found: false,
    };
    finder.visit_type(ty);
    finder.found
}

fn cfg_implies_test_attribute(attribute: &Attribute) -> bool {
    if !attribute.path().is_ident("cfg") {
        return false;
    }
    attribute
        .parse_args::<syn::Meta>()
        .is_ok_and(|meta| meta_implies_test(&meta))
}

fn meta_implies_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => {
            parse_nested_meta(list).is_some_and(|nested| nested.iter().any(meta_implies_test))
        }
        syn::Meta::List(list) if list.path.is_ident("any") => parse_nested_meta(list)
            .is_some_and(|nested| !nested.is_empty() && nested.iter().all(meta_implies_test)),
        syn::Meta::List(_) | syn::Meta::NameValue(_) => false,
    }
}

fn parse_nested_meta(list: &syn::MetaList) -> Option<Vec<syn::Meta>> {
    use syn::parse::Parser as _;
    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse2(list.tokens.clone())
        .ok()
        .map(|nested| nested.into_iter().collect())
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => &[],
        _ => &[],
    }
}

fn impl_item_attributes(item: &ImplItem) -> &[Attribute] {
    match item {
        ImplItem::Const(item) => &item.attrs,
        ImplItem::Fn(item) => &item.attrs,
        ImplItem::Type(item) => &item.attrs,
        ImplItem::Macro(item) => &item.attrs,
        ImplItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn trait_item_attributes(item: &TraitItem) -> &[Attribute] {
    match item {
        TraitItem::Const(item) => &item.attrs,
        TraitItem::Fn(item) => &item.attrs,
        TraitItem::Type(item) => &item.attrs,
        TraitItem::Macro(item) => &item.attrs,
        TraitItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn foreign_item_attributes(item: &ForeignItem) -> &[Attribute] {
    match item {
        ForeignItem::Fn(item) => &item.attrs,
        ForeignItem::Static(item) => &item.attrs,
        ForeignItem::Type(item) => &item.attrs,
        ForeignItem::Macro(item) => &item.attrs,
        ForeignItem::Verbatim(_) => &[],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_fields() -> BTreeSet<String> {
        ["items", "thumbnails", "visible_indices", "selected"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn fixture(source: &str) -> Vec<Violation> {
        analyze_source("src/fixture.rs", source, &bundle_fields(), false).unwrap()
    }

    fn api_allowlist(source: &str) -> Vec<String> {
        extract_public_api_fingerprints(source)
            .unwrap()
            .into_iter()
            .map(|fingerprint| fingerprint.text)
            .collect()
    }

    fn has_rule(violations: &[Violation], rule: Rule) -> bool {
        violations.iter().any(|violation| violation.rule == rule)
    }

    fn sample_violation(file: &str, function: &str, rule: Rule) -> Violation {
        Violation {
            rule,
            file: file.to_owned(),
            function: function.to_owned(),
            line: 7,
            message: "fixture violation detail".into(),
        }
    }

    fn assert_flagged_and_clear(rule: Rule, flagged: &str, clear: &str) {
        let flagged_violations = fixture(flagged);
        assert!(
            has_rule(&flagged_violations, rule),
            "expected {rule}, got {flagged_violations:#?}"
        );
        let clear_violations = fixture(clear);
        assert!(
            !has_rule(&clear_violations, rule),
            "did not expect {rule}, got {clear_violations:#?}"
        );
    }

    #[test]
    fn field_extraction_reads_named_fields_from_the_struct() {
        let fields = extract_bundle_fields(
            "struct ViewerContextBundle { items: Vec<u8>, selected: Option<usize> }",
        )
        .unwrap();
        assert_eq!(
            fields,
            ["items", "selected"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn field_extraction_fails_when_the_struct_is_missing() {
        let error = extract_bundle_fields("struct SomethingElse { items: Vec<u8> }").unwrap_err();
        assert!(error.contains("was not found"), "{error}");
    }

    #[test]
    fn a4_accepts_the_exact_expected_surface() {
        let source =
            "pub(crate) struct Demo; impl Demo { pub(crate) fn run<T>(&self, value: T) {} }";
        let allowlist = api_allowlist(source);
        let refs = allowlist.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(audit_public_api(source, &refs).unwrap().is_empty());
    }

    #[test]
    fn a4_rejects_a_trait_impl_addition() {
        let allowed = "pub(crate) struct Demo;";
        let allowlist = api_allowlist(allowed);
        let refs = allowlist.iter().map(String::as_str).collect::<Vec<_>>();
        let changed = "pub(crate) struct Demo; impl Drop for Demo { fn drop(&mut self) {} }";
        assert!(has_rule(
            &audit_public_api(changed, &refs).unwrap(),
            Rule::A4
        ));
    }

    #[test]
    fn a4_rejects_a_generic_bound_addition() {
        let allowed =
            "pub(crate) struct Demo; impl Demo { pub(crate) fn run<T>(&self, value: T) {} }";
        let allowlist = api_allowlist(allowed);
        let refs = allowlist.iter().map(String::as_str).collect::<Vec<_>>();
        let changed =
            "pub(crate) struct Demo; impl Demo { pub(crate) fn run<T: Clone>(&self, value: T) {} }";
        assert!(has_rule(
            &audit_public_api(changed, &refs).unwrap(),
            Rule::A4
        ));
    }

    #[test]
    fn a4_rejects_visibility_broadening() {
        let allowed = "pub(crate) struct Demo;";
        let allowlist = api_allowlist(allowed);
        let refs = allowlist.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(has_rule(
            &audit_public_api("pub struct Demo;", &refs).unwrap(),
            Rule::A4
        ));
    }

    #[test]
    fn a6_rejects_unguarded_test_definitions_and_calls() {
        let source = r#"
            impl App {
                fn helper_for_test(&mut self) {}
            }
            fn production(app: &mut App) {
                app.helper_for_test();
                free_for_test();
            }
        "#;
        let violations = analyze_test_api("src/fixture.rs", source, false).unwrap();
        assert!(
            violations
                .iter()
                .filter(|violation| violation.rule == Rule::A6)
                .count()
                >= 3,
            "{violations:#?}"
        );
    }

    #[test]
    fn a6_accepts_the_existing_cfg_all_test_windows_app_impl_shape() {
        let source = r#"
            #[cfg(all(test, windows))]
            impl App {
                pub(in crate::app) fn helper_for_test(&mut self) {}
            }
            #[cfg(test)]
            fn test_only(app: &mut App) {
                app.helper_for_test();
            }
        "#;
        let violations = analyze_test_api("src/fixture.rs", source, false).unwrap();
        assert!(!has_rule(&violations, Rule::A6), "{violations:#?}");
    }

    #[test]
    fn a2a_flags_bundle_field_swap_but_not_an_unrelated_field() {
        assert_flagged_and_clear(
            Rule::A2a,
            r#"fn move_it(app: &mut App, stash: &mut Vec<u8>) {
                std::mem::swap(&mut app.items, stash);
            }"#,
            r#"fn move_it(app: &mut App, stash: &mut Vec<u8>) {
                std::mem::swap(&mut app.unrelated, stash);
            }"#,
        );
    }

    #[test]
    fn a2b_flags_three_distinct_fields_but_allows_two() {
        assert_flagged_and_clear(
            Rule::A2b,
            r#"fn move_it(app: &mut App) {
                std::mem::take(&mut app.items);
                std::mem::take(&mut app.thumbnails);
                std::mem::take(&mut app.visible_indices);
            }"#,
            r#"fn move_it(app: &mut App) {
                std::mem::take(&mut app.items);
                std::mem::take(&mut app.thumbnails);
                std::mem::take(&mut app.items);
            }"#,
        );
    }

    #[test]
    fn a2b_counts_each_function_separately() {
        let source = r#"
            fn first(app: &mut App) {
                std::mem::take(&mut app.items);
                std::mem::take(&mut app.thumbnails);
            }
            fn second(app: &mut App) {
                std::mem::take(&mut app.visible_indices);
                std::mem::take(&mut app.selected);
            }
        "#;
        let split = fixture(source);
        assert!(!has_rule(&split, Rule::A2b), "{split:#?}");

        let combined = fixture(
            r#"fn combined(app: &mut App) {
                std::mem::take(&mut app.items);
                std::mem::take(&mut app.thumbnails);
                std::mem::take(&mut app.visible_indices);
            }"#,
        );
        assert!(has_rule(&combined, Rule::A2b), "{combined:#?}");
    }

    #[test]
    fn a2_normalizes_renamed_take_imports_before_thresholding() {
        assert_flagged_and_clear(
            Rule::A2b,
            r#"use std::mem::take as pull;
            fn move_it(app: &mut App) {
                pull(&mut app.items);
                pull(&mut app.thumbnails);
                pull(&mut app.visible_indices);
            }"#,
            r#"use another::take as pull;
            fn move_it(app: &mut App) {
                pull(&mut app.items);
                pull(&mut app.thumbnails);
                pull(&mut app.visible_indices);
            }"#,
        );
    }

    #[test]
    fn a2_normalizes_module_aliases_groups_globs_and_core_paths() {
        for source in [
            r#"use std::mem as memory;
                fn move_it(app: &mut App) {
                    memory::take(&mut app.items);
                    memory::replace(&mut app.thumbnails, Vec::new());
                    memory::take(&mut app.visible_indices);
                }"#,
            r#"use core::mem::{take, replace};
                fn move_it(app: &mut App) {
                    take(&mut app.items);
                    replace(&mut app.thumbnails, Vec::new());
                    take(&mut app.visible_indices);
                }"#,
            r#"use std::mem::*;
                fn move_it(app: &mut App) {
                    take(&mut app.items);
                    replace(&mut app.thumbnails, Vec::new());
                    take(&mut app.visible_indices);
                }"#,
        ] {
            let violations = fixture(source);
            assert!(has_rule(&violations, Rule::A2b), "{violations:#?}");
        }
    }

    #[test]
    fn a3_flags_associated_calls_but_skips_cfg_test_items() {
        assert_flagged_and_clear(
            Rule::A3,
            "fn bad() { ViewerContextBundle::empty(); }",
            "#[cfg(test)] fn allowed() { ViewerContextBundle::empty(); }",
        );
    }

    #[test]
    fn a3_skips_items_nested_below_cfg_test_but_not_the_tests_file() {
        let cfg_source = r#"
            #[cfg(test)]
            impl Demo {
                fn allowed() { ViewerContextBundle::empty(); }
            }
        "#;
        let cfg_violations = fixture(cfg_source);
        assert!(!has_rule(&cfg_violations, Rule::A3), "{cfg_violations:#?}");

        let ordinary = analyze_source(
            "src/app/tests.rs",
            "fn helper() { ViewerContextBundle::empty(); }",
            &bundle_fields(),
            false,
        )
        .unwrap();
        assert!(has_rule(&ordinary, Rule::A3), "{ordinary:#?}");
    }

    #[test]
    fn a1_flags_type_positions_import_aliases_and_reexports_but_not_value_paths_or_text() {
        for source in [
            "struct Holder { bundle: Option<Box<ViewerContextBundle>> }",
            "fn take(bundle: &mut ViewerContextBundle) {}",
            "fn make() -> Vec<ViewerContextBundle> { todo!() }",
            "fn local() { let value: ViewerContextBundle = todo!(); }",
            "impl Demo<ViewerContextBundle> {}",
            "type BundleAlias = ViewerContextBundle;",
            "use crate::app::viewer_context_registry::ViewerContextBundle as BundleAlias;",
            "pub use crate::app::viewer_context_registry::ViewerContextBundle;",
        ] {
            let violations = fixture(source);
            assert!(has_rule(&violations, Rule::A1), "{source}: {violations:#?}");
        }

        for source in [
            "fn value_path_only() { ViewerContextBundle::empty(); }",
            r#"#[doc = "ViewerContextBundle"] fn documented() {}"#,
            r#"const NAME: &str = "ViewerContextBundle";"#,
        ] {
            let violations = fixture(source);
            assert!(
                !has_rule(&violations, Rule::A1),
                "{source}: {violations:#?}"
            );
        }
    }

    #[test]
    fn a5_flags_legacy_ownership_identifiers_but_not_text() {
        for source in [
            "fn bad(active_detached_viewer_context: usize) {}",
            "fn bad(app: App) { app.paused_bundle.take(); }",
        ] {
            let violations = fixture(source);
            assert!(has_rule(&violations, Rule::A5), "{source}: {violations:#?}");
        }
        for source in [
            r#"#[doc = "paused_bundle"] fn documented() {}"#,
            r#"const NAME: &str = "active_detached_viewer_context";"#,
        ] {
            let violations = fixture(source);
            assert!(
                !has_rule(&violations, Rule::A5),
                "{source}: {violations:#?}"
            );
        }
    }

    #[test]
    fn a7_shape_a_mem_operation_target() {
        assert_flagged_and_clear(
            Rule::A7a,
            "fn bad(app: &mut App) { std::mem::take(&mut app.viewer_contexts); }",
            "fn okay(app: &mut App) { std::mem::take(&mut app.other); }",
        );
    }

    #[test]
    fn a7_shape_a_uses_the_shared_import_normalizer() {
        assert_flagged_and_clear(
            Rule::A7a,
            r#"use core::mem::take as pull;
                fn bad(app: &mut App) { pull(&mut app.viewer_contexts); }"#,
            r#"use elsewhere::take as pull;
                fn okay(app: &mut App) { pull(&mut app.viewer_contexts); }"#,
        );
    }

    #[test]
    fn a7_shape_b_assignment_target() {
        assert_flagged_and_clear(
            Rule::A7b,
            "fn bad(app: &mut App) { app.viewer_contexts = ViewerContextRegistry::new(); }",
            "fn okay(app: &mut App) { app.other = ViewerContextRegistry::new(); }",
        );
    }

    #[test]
    fn a7_shape_c_mutable_borrow() {
        assert_flagged_and_clear(
            Rule::A7c,
            "fn bad(app: &mut App) { helper(&mut app.viewer_contexts); }",
            "fn okay(app: &mut App) { helper(&mut app.other); }",
        );
    }

    #[test]
    fn a7_shape_d_value_destructure() {
        assert_flagged_and_clear(
            Rule::A7d,
            "fn bad(app: App) { let App { viewer_contexts, .. } = app; consume(viewer_contexts); }",
            "fn okay(app: App) { let App { ref viewer_contexts, .. } = app; inspect(viewer_contexts); }",
        );
    }

    #[test]
    fn a7_shape_e_naked_value_move() {
        assert_flagged_and_clear(
            Rule::A7e,
            "fn bad(app: App) { let registry = app.viewer_contexts; consume(registry); }",
            "fn okay(app: &App) { let registry = &app.viewer_contexts; inspect(registry); }",
        );
    }

    #[test]
    fn a7_shape_e_allows_method_and_member_access() {
        let direct_move = fixture("fn bad(app: App) { consume(app.viewer_contexts); }");
        assert!(has_rule(&direct_move, Rule::A7e), "{direct_move:#?}");
        let access = fixture(
            "fn okay(app: &mut App) { app.viewer_contexts.mount(); let n = app.viewer_contexts.len; }",
        );
        assert!(!has_rule(&access, Rule::A7e), "{access:#?}");
    }

    #[test]
    fn a7_shape_f_registry_in_return_type() {
        assert_flagged_and_clear(
            Rule::A7f,
            "fn take_registry() -> Option<Box<ViewerContextRegistry>> { todo!() }",
            "fn take_registry() -> Option<Box<OtherRegistry>> { todo!() }",
        );
    }

    #[test]
    fn registry_module_suppresses_every_rule_after_the_same_source_rejects_elsewhere() {
        let source = r#"
            use std::mem::{swap, take};
            fn all(
                app: &mut App,
                bundle: &ViewerContextBundle,
            ) -> Option<ViewerContextRegistry> {
                let paused_bundle = bundle;
                swap(&mut app.selected, &mut stash);
                take(&mut app.items);
                take(&mut app.thumbnails);
                take(&mut app.visible_indices);
                ViewerContextBundle::empty();
                take(&mut app.viewer_contexts);
                app.viewer_contexts = ViewerContextRegistry::new();
                helper(&mut app.viewer_contexts);
                let App { viewer_contexts, .. } = *app;
                consume(viewer_contexts);
                consume(app.viewer_contexts);
                todo!()
            }
        "#;
        let outside = fixture(source);
        for rule in [
            Rule::A1,
            Rule::A2a,
            Rule::A2b,
            Rule::A3,
            Rule::A5,
            Rule::A7a,
            Rule::A7b,
            Rule::A7c,
            Rule::A7d,
            Rule::A7e,
            Rule::A7f,
        ] {
            assert!(has_rule(&outside, rule), "missing {rule}: {outside:#?}");
        }

        let inside = analyze_source(REGISTRY_PATH, source, &bundle_fields(), true).unwrap();
        assert!(inside.is_empty(), "{inside:#?}");
    }

    #[test]
    fn allowlist_requires_a_reason_and_rejects_stale_entries() {
        let missing_reason = [AllowlistEntry {
            file: "src/fixture.rs",
            function: "bad",
            rule: Rule::A3,
            reason: " ",
        }];
        assert!(validate_allowlist(&missing_reason).is_err());

        let stale = [AllowlistEntry {
            file: "src/fixture.rs",
            function: "bad",
            rule: Rule::A3,
            reason: "fixture reason",
        }];
        assert!(classify_findings(Vec::new(), &stale, &[], true).is_err());

        let violations = fixture("fn bad() { ViewerContextBundle::empty(); }");
        assert!(has_rule(&violations, Rule::A3));
        let report = classify_findings(violations, &stale, &[], true).unwrap();
        assert!(report.violations.is_empty());
    }

    #[test]
    fn known_finding_is_reported_without_failing_the_run() {
        let known = [KnownFindingEntry {
            file: "src/fixture.rs",
            function: "known_move",
            rule: Rule::A2b,
            reason: "Moves per-context fixture state into a global fixture slot.",
            tracking: "docs/detached-rework-plan.md",
        }];
        let report = classify_findings(
            vec![sample_violation("src/fixture.rs", "known_move", Rule::A2b)],
            &[],
            &known,
            true,
        )
        .unwrap();
        let output = render_report(report);
        assert_eq!(output.exit_code, 0, "{}", output.text);
        assert!(
            output
                .text
                .contains("KNOWN FINDING A2b src/fixture.rs:7 known_move"),
            "{}",
            output.text
        );
        assert!(output.text.contains(known[0].reason), "{}", output.text);
        assert!(output.text.contains(known[0].tracking), "{}", output.text);
        assert!(
            output.text.contains("no untracked violations"),
            "{}",
            output.text
        );
    }

    #[test]
    fn known_finding_that_stops_matching_is_a_named_error() {
        let known = [KnownFindingEntry {
            file: "src/fixture.rs",
            function: "known_move",
            rule: Rule::A2b,
            reason: "Fixture reason.",
            tracking: "docs/detached-rework-plan.md",
        }];
        let error = classify_findings(Vec::new(), &[], &known, true).unwrap_err();
        assert!(error.contains("known finding stopped matching"), "{error}");
        assert!(error.contains("src/fixture.rs"), "{error}");
        assert!(error.contains("known_move"), "{error}");
        assert!(error.contains("A2b"), "{error}");
    }

    #[test]
    fn known_finding_requires_reason_and_tracking_reference() {
        let missing_reason = [KnownFindingEntry {
            file: "src/fixture.rs",
            function: "known_move",
            rule: Rule::A2b,
            reason: " ",
            tracking: "docs/detached-rework-plan.md",
        }];
        let reason_error = validate_known_findings(&missing_reason).unwrap_err();
        assert!(reason_error.contains("has no reason"), "{reason_error}");

        let missing_tracking = [KnownFindingEntry {
            file: "src/fixture.rs",
            function: "known_move",
            rule: Rule::A2b,
            reason: "Fixture reason.",
            tracking: " ",
        }];
        let tracking_error = validate_known_findings(&missing_tracking).unwrap_err();
        assert!(
            tracking_error.contains("has no tracking reference"),
            "{tracking_error}"
        );
    }

    #[test]
    fn no_allowlist_flag_reports_each_allowlisted_entry() {
        let options = parse_cli_args(["--no-allowlist"]).unwrap();
        assert!(!options.use_allowlist);

        let mut violations: Vec<_> = ALLOWLIST_ENTRIES
            .iter()
            .map(|entry| sample_violation(entry.file, entry.function, entry.rule))
            .collect();
        violations.extend(
            KNOWN_FINDINGS
                .iter()
                .map(|entry| sample_violation(entry.file, entry.function, entry.rule)),
        );
        let report = classify_findings(
            violations,
            ALLOWLIST_ENTRIES,
            KNOWN_FINDINGS,
            options.use_allowlist,
        )
        .unwrap();

        for entry in ALLOWLIST_ENTRIES {
            assert!(
                report.violations.iter().any(|violation| {
                    violation.file == entry.file
                        && violation.function == entry.function
                        && violation.rule == entry.rule
                }),
                "missing {} / {} / {}: {:#?}",
                entry.file,
                entry.function,
                entry.rule,
                report.violations
            );
        }
        let output = render_report(report);
        assert_eq!(output.exit_code, 1, "{}", output.text);
        for entry in ALLOWLIST_ENTRIES {
            assert!(output.text.contains(entry.file), "{}", output.text);
            assert!(output.text.contains(entry.function), "{}", output.text);
        }
        // 既知の指摘の描画・失敗条件は合成データの
        // `known_finding_is_reported_without_failing_the_run` /
        // `known_finding_that_stops_matching_is_a_named_error` が持っている。
        // ここで実リストが空でないことを要求すると、**指摘を 1 件も抱えていない
        // 健全な状態でテストが落ちる** (実際に `activate_snapshot` の解決で落ちた)。
    }
}
