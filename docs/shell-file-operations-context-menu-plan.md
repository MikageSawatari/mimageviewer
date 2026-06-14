# Shell file operations and native context menu plan

Status: mixed. Folder A/B quick workspaces and the real-file native Shell
context menu are implemented. `IFileOperation` copy/move/delete replacement and
Win32 custom menus for ZIP/PDF virtual items remain for later implementation
sessions.

## 1. Goal

mIV currently uses egui for the grid context menu and a mix of PowerShell,
`SHFileOperationW`, and Shell `IDataObject` helpers for file organization. The
goal of this plan is to move real file/folder organization toward Windows Shell
behavior without replacing the egui/wgpu viewer UI.

Target outcome:

- Real filesystem items use Windows Shell operations for copy, cut, paste,
  delete, rename, properties, and standard "Open with" behavior.
- Real filesystem item context menus are native Win32 popup menus that combine
  a small mIV command section with the Windows Shell context menu.
- The folder bar offers A/B quick folder workspaces so file organization can
  move between a source and destination without turning the whole app into a
  two-pane file manager.
- ZIP/PDF virtual items use a Windows-native custom popup menu, not the Shell
  context menu, because they do not have independent filesystem identity.
- egui context menus remain only as a fallback for non-Windows builds or Shell
  integration failure.
- Existing mIV-specific workflows such as representative thumbnail pinning,
  image-to-clipboard, virtual path copy, search-result jump, and non-destructive
  rotation stay available.

Non-goals:

- Do not replace egui or the wgpu thumbnail/fullscreen rendering stack.
- Do not implement a full two-pane file manager in this phase. A/B quick
  folders switch one active grid between two workspaces; they do not create two
  simultaneous selections, two visible grids, or drag targets inside mIV.
- Do not expose destructive file operations for ZIP entries or PDF pages.
- Do not implement a full Explorer view or `IExplorerBrowser` host in this
  phase.
- Do not move legacy XMP tag import into the native context menu. Keep it in
  the top-level tag menu.

## 2. Related existing design

Read these first in the implementation session:

- `docs/architecture-overview.md`
- `docs/file-drag-drop-design.md`
- `docs/ui-responsiveness.md`
- `docs/virtual-folders.md`
- `docs/spec.md`
- `src/file_drag.rs`
- `src/delete_worker.rs`
- `src/ui_dialogs/context_menu.rs`
- `src/grid_item.rs`

Useful existing facts:

- `src/file_drag.rs` already creates Shell `IDataObject` values from paths and
  calls `SHDoDragDrop`.
- `src/delete_worker.rs` already moves files to the recycle bin with
  `SHFileOperationW`, one path at a time.
- `src/ui_dialogs/context_menu.rs` currently uses PowerShell for file
  copy/cut/paste and skips folders for paste/drop because collision handling and
  recursive folder copies are unsafe in the current implementation.
- `src/app.rs` currently keeps one folder navigation back stack, one forward
  stack, one global recent-folder list, and one `suppress_folder_nav_record_once`
  flag. A/B quick folders should factor this into reusable history state rather
  than duplicating ad hoc path stacks.
- `GridItem::file_operation_path()` currently excludes folders, while
  `GridItem::drag_source_path()` includes folders.
- ZIP/PDF virtual items (`ZipImage`, `ZipDir`, `PdfPage`) have stable mIV
  identity keys but not independent filesystem paths.

## 3. Item routing

| Context | Target menu | Shell file operations | Notes |
| --- | --- | --- | --- |
| `Image`, `Video`, `ZipFile`, `PdfFile`, `ConvertibleArchive` | Native mIV + Shell menu | Yes | Real file paths. |
| `Folder` tile or background current folder | Native mIV + Shell menu | Yes | Re-enable folder copy/paste/delete only through Shell-safe paths. |
| `SearchContainer` | egui fallback for now | Maybe later | The stored path is real, but search views need extra "go to folder" semantics and must not paste into stale `current_folder`. |
| Checked selection, all real paths | Native mIV + Shell menu | Yes | Include folders only after selection model is updated to allow folder checks. |
| Checked selection mixed real + virtual | egui fallback for now | No Shell menu | Avoid silently applying Shell verbs to only part of the selection. |
| `ZipImage` | Native mIV custom menu | No | May offer copy rendered image and virtual path only. |
| `PdfPage` | Native mIV custom menu | No | May add rendered page image copy as a future command. |
| `ZipDir` | Native mIV custom menu | No | Virtual nested book/container. |
| `ZipSeparator` | No menu | No | Legacy pseudo item, currently not generated. |

## 4. mIV command inventory

Keep in the context menu:

- Representative thumbnail pin/unpin.
- Copy image to clipboard.
- Copy virtual path or real path.
- Copy file name or page name.
- Jump to containing folder from search/tag result views.
- Non-destructive rotate left/right for image-like items.

Move to Shell:

- Copy.
- Cut.
- Paste.
- Delete.
- Rename.
- Properties.
- Open / Open with / Send to / Share.
- Real folder/file operations generally.

Remove from context menu:

- Legacy XMP tag import/remove. It already belongs in the top-level tag menu and
  makes the file context menu too dense.

## 5. Folder A/B quick workspaces

Implementation status: implemented before the Shell operation phases.

Purpose:

- Make copy-source / paste-destination workflows practical after Shell file
  operations are added.
- Avoid the much larger two-pane refactor. The app still has one visible grid,
  one active item list, one selection model, and one thumbnail pipeline.

Data model:

```rust
pub enum QuickFolderSlotId {
    A,
    B,
}

pub struct FolderNavHistoryState {
    pub back_stack: Vec<PathBuf>,
    pub forward_stack: Vec<PathBuf>,
    pub suppress_record_once: bool,
}

pub struct QuickFolderWorkspace {
    pub target: Option<PathBuf>,
    pub history: FolderNavHistoryState,
}
```

Implementation notes:

- Add two quick workspaces, A and B. A is active by default; users switch
  between A and B rather than registering one-off bookmarks.
- Keep the existing non-A/B navigation state only as a compatibility/fallback
  implementation detail. Normal visible navigation uses the active A/B
  workspace.
- Persist only the two workspace `target` paths in `Settings`, for example as
  `[Option<PathBuf>; 2]`. Keep each slot's back/forward history session-local,
  matching the current folder history behavior.
- Keep `recent_folders` global. It remains the "recent places" MRU and is not
  split per A/B workspace.
- Use the user-visible navigation target, not an implementation cache path:
  - real folder: the folder path,
  - ZIP/PDF/convertible archive root: the source container path,
  - converted RAR/7z/LZH cache: the original archive path via the existing
    source override/effective-folder logic.
- Do not store ZIP entry paths, PDF page identities, search-result virtual
  paths, or tag/favorite temporary views in A/B slots.

Folder history behavior:

- A/B switching itself must not push anything onto any folder history stack.
  Switching from A to B is a workspace change, not a folder navigation.
- While A is active, direct address-bar navigation, parent navigation,
  tree-order navigation, favorite navigation, and Shell-menu "go to folder"
  transitions update A's target and A's back/forward stacks.
- While B is active, the same operations update B's target and B's back/forward
  stacks.
- At startup the active slot is A. Clearing remembered quick locations resets
  the active slot to A.
- The left/right folder-history buttons read and mutate the active history
  state: A's buttons operate on A history, B's on B history, and normal mode
  uses the existing normal history.
- Existing snapshot/rollback helpers used by search and archive conversion
  should snapshot all three history states (normal, A, B), the active slot, and
  `recent_folders`, so failed or canceled temporary navigation can restore the
  exact pre-operation state.
- Pending folder DFS navigation should be canceled before switching active A/B
  slots, as with other explicit folder loads.

Folder bar UI:

- Place compact `A` and `B` buttons near the existing left/right history
  buttons.
- Left-click: switch to that workspace. If the workspace has a remembered
  target, load it; if it has no remembered target yet, show the drive list.
- Opening a real folder / ZIP / PDF / convertible archive while A or B is
  active automatically updates that workspace's remembered target.
- Highlight the active slot. If the current effective target equals a
  registered inactive slot, show a softer "same target" state so users can see
  why clicking it would not visibly move.
- Disable A/B switching while search/favorite/tag temporary views are active.
  After a user explicitly jumps from a search result into a real folder, A/B
  works normally again.
- If a registered target no longer exists, leave the slot unchanged, show a
  toast, and fall back to the drive list. Do not silently overwrite the slot
  with a parent fallback.

Settings:

- Add `show_address_bar_quick_folders`, default on, beside the existing folder
  bar visibility settings.
- Add `quick_folder_slots`, default `[None, None]`.
- Add a clear/reset control in the toolbar preferences page near the recent
  folder history clear control.

## 6. `IFileOperation` layer

Add a Windows-only module, tentatively `src/shell_file_ops.rs`.

Public API sketch:

```rust
pub enum ShellFileOpRequest {
    CopyToFolder { sources: Vec<PathBuf>, dest_folder: PathBuf },
    MoveToFolder { sources: Vec<PathBuf>, dest_folder: PathBuf },
    Delete { targets: Vec<PathBuf>, recycle: bool },
    Rename { target: PathBuf, new_name: String },
    PasteClipboardToFolder { dest_folder: PathBuf },
}

pub struct ShellFileOpOutcome {
    pub op: ShellFileOpKind,
    pub hresult: i32,
    pub aborted: bool,
    pub touched_roots: Vec<PathBuf>,
    pub message: Option<String>,
}
```

Execution model:

- Run operations on a dedicated STA worker thread, not inside `App::update`.
- Initialize COM on that worker with `COINIT_APARTMENTTHREADED`.
- Call `IFileOperation::SetOwnerWindow(main_hwnd)` so progress, collision, and
  error dialogs are owned by the mIV window.
- Call `IFileOperation::SetOperationFlags(...)` before
  `PerformOperations`.
- Use standard Shell UI by default:
  - Do not set `FOF_SILENT`.
  - Do not set `FOF_NOCONFIRMATION`.
  - Do not set `FOF_NOERRORUI`.
  - Do not set `FOF_RENAMEONCOLLISION` unless a future setting asks for
    automatic rename-on-collision.
- For recycle-bin delete, use the Shell recycle behavior and keep the current
  safety principle: if the operation can become permanent deletion, the user
  must see a warning.
- After `PerformOperations`, call `GetAnyOperationsAborted`. `PerformOperations`
  can return success even when the user canceled.
- Return a compact outcome to UI. Use mIV toasts only for summaries or failures
  that did not already show Shell UI.

Progress:

- MVP uses the standard Shell progress/collision/error UI.
- Do not build a parallel egui progress dialog for copy/move/delete in the first
  pass.
- `IFileOperationProgressSink` is optional. Add it only when mIV needs exact
  per-item completion data, custom logging, or richer post-operation reload
  decisions. The standard UI does not require it.

Reload behavior:

- On successful or partially successful operations, reload the affected visible
  folder if it matches `current_favorite_target()` or `effective_folder()`.
- For search/tag result views, do not paste into the saved pre-search folder.
  Continue to reject or explicitly route the operation to a user-visible target.
- For delete/move, also clear stale checked indices after the reload.

## 7. Clipboard strategy

The target is to remove PowerShell from copy/cut/paste, but it can be phased.

Current status:

- Ctrl+C/Ctrl+X on real filesystem selections invokes Shell `copy`/`cut`
  verbs through `src/native_context_menu.rs`.
- Ctrl+V invokes the current real folder's Shell background `paste` verb. Folder
  paste, collision prompts, and progress UI are therefore handled by Windows.
- The currently displayed real folder is watched with notify-rs
  (`ReadDirectoryChangesW`) and debounced into `check_external_folder_changes`,
  so Shell menu operations, Ctrl+V paste, and external Explorer edits share the
  same refresh path.
- The legacy PowerShell clipboard helpers still exist for egui fallback menus
  and drop/copy internals until the `IFileOperation` migration is complete.

Remaining sequence:

1. Replace drop-to-folder and egui fallback paste internals with
   `IFileOperation`. This removes the remaining PowerShell path and keeps
   folder support, collision prompts, progress UI, and permanent-delete safety
   where it matters most.
2. If a direct Shell clipboard writer is needed later, use the existing
   `IDataObject` path-building logic from `src/file_drag.rs` and add/override
   `CFSTR_PREFERREDDROPEFFECT` for cut. This likely requires a wrapper
   `IDataObject`; avoid that until the verb route is proven insufficient.

Keep the existing image clipboard code separate:

- File data clipboard (`CF_HDROP` / Shell data object) is for file
  organization.
- Image pixel clipboard (`CF_DIB`) is for "copy image to clipboard".
- Preserve the existing clipboard sequence guard so slow image decode cannot
  overwrite a newer clipboard action.

## 8. Native context menu architecture

Implementation status: implemented for the main grid real-file path. The helper
is `src/native_context_menu.rs`, and `src/ui_dialogs/context_menu.rs` routes
real files/folders to it when `Settings::use_native_shell_context_menu` is
enabled. The implementation inserts mIV commands first, asks Shell to populate
the rest, uses `TrackPopupMenuEx(..., TPM_RETURNCMD, ...)`, dispatches Shell
commands with `IContextMenu::InvokeCommand`, and temporarily subclasses the main
HWND to forward `IContextMenu2` / `IContextMenu3` owner-draw and submenu
messages.

Current routing details:

- Real file/folder tiles use `IShellItemArray::BindToHandler(BHID_SFUIObject)`
  for Shell commands.
- Background right-click on the current real folder uses
  `IShellFolder::CreateViewObject(IContextMenu)` so Paste/New-style background
  verbs come from Shell instead of treating the current folder as a deletable
  selected object.
- Background right-click also inserts an mIV "貼り付け" command before Shell
  items. It invokes the same canonical Shell `paste` verb as Ctrl+V, covering
  environments where the Shell-populated background menu does not surface Paste.
- Keyboard Ctrl+C/Ctrl+X/Ctrl+V use the same native helper and canonical Shell
  `copy`/`cut`/`paste` verbs instead of mIV's custom clipboard writer.
- After Shell verbs and native context menus return, the App resynchronizes
  egui's current Ctrl/Shift/Alt modifier state from Win32 physical key state.
  The same sync also runs before normal frame input handling on Windows, so a
  Shell modal loop that misses a KeyUp cannot leave Ctrl "stuck" in mIV.
- Native context menu latency is instrumented under the `native_menu` perf-log
  category. `app_prepare` covers mIV-side routing and command construction;
  `show_shell_bind`, `show_query_shell`, and `show_pre_track` cover the Shell
  setup before the popup is shown; `show_track_popup_block` includes the
  user-visible popup lifetime; `show_invoke_shell` and `verb_*` cover Shell
  command invocation paths. Slow stages (120 ms or more, 80 ms for
  `app_prepare`) are also summarized in the normal log.
- Checked selections use the Shell menu only when every checked item has a real
  file-operation path. Mixed real + ZIP/PDF virtual selections keep the egui
  fallback.
- ZIP/PDF pages, ZIP directories, search containers, and other virtual items
  still use the egui fallback until the Win32 custom virtual menu is implemented.

Windows-only module: `src/native_context_menu.rs`.

Responsibilities:

- Classify a grid/menu request into real-filesystem, virtual-item, mixed, or
  fallback.
- Build a native `HMENU`.
- Insert mIV custom commands with IDs outside the Shell command ID range.
- Ask the Shell `IContextMenu` to insert its commands.
- Track the popup menu and dispatch the selected command.
- Return an `App`-level command for mIV actions; invoke Shell actions directly.

Menu ID plan:

- Reserve `0x7000..0x70ff` for mIV commands.
- Offer Shell commands an ID range below that, for example
  `idCmdFirst = 1`, `idCmdLast = 0x6fff`.
- If a selected ID is in the mIV range, dispatch internally.
- Otherwise pass `selected_id - idCmdFirst` to `IContextMenu::InvokeCommand`.

Real filesystem menu construction:

1. Create `HMENU` with `CreatePopupMenu`.
2. Insert mIV custom commands at the top.
3. Insert a separator.
4. Obtain `IContextMenu` for the selected real paths.
5. Call `IContextMenu::QueryContextMenu` at the current insertion point.
6. Show with `TrackPopupMenuEx(..., TPM_RETURNCMD, ...)`.
7. Dispatch selected command.

`IContextMenu` retrieval:

- Preferred for same-parent selections: bind the parent `IShellFolder`, convert
  each selected child to a relative PIDL, and call
  `IShellFolder::GetUIObjectOf(..., IID_IContextMenu, ...)`.
- Spike for cross-folder selections: test `IShellItemArray::BindToHandler` with
  `BHID_SFUIObject` and `IContextMenu`.
- If cross-folder Shell menus are unreliable, fall back to a mIV custom menu
  with common operations only.

Message forwarding:

- Some Shell extensions require `IContextMenu2` / `IContextMenu3` message
  forwarding for owner-drawn menu items and submenus.
- During `TrackPopupMenuEx`, temporarily subclass the main HWND or otherwise
  hook the relevant window messages and forward `WM_INITMENUPOPUP`,
  `WM_DRAWITEM`, `WM_MEASUREITEM`, and `WM_MENUCHAR` to
  `IContextMenu3::HandleMenuMsg2` or `IContextMenu2::HandleMenuMsg`.
- Restore the original WndProc immediately after the menu closes.
- If this proves too fragile with winit/eframe, gate the Shell menu behind a
  setting and keep the egui fallback.

Settings:

- `use_native_shell_context_menu` is implemented and defaults on so the Shell
  menu is visible in development builds immediately. Users can disable it from
  Settings -> Explorer integration if a Shell extension behaves badly.
- Add a second setting if needed later: `show_miv_commands_in_native_menu`,
  default on.
- If a Shell menu crashes/hangs reports appear, users can disable the native
  Shell menu without losing mIV core functionality.

## 9. Native custom menu for virtual items

Do not use the Shell context menu for virtual items. Instead, use the same
Win32 `HMENU` style but populate it only with mIV commands. This keeps the menu
visually close to the real-file native menu without pretending the virtual item
is a filesystem object.

`ZipImage` menu:

```text
ZIP image: page001.jpg   (disabled header)
------------------------------------------
Copy image to clipboard
Copy virtual path
Copy file name
Representative thumbnail: pin/unpin
------------------------------------------
Show source ZIP in Explorer
Copy source ZIP path
```

`PdfPage` menu:

```text
PDF page: Page 12        (disabled header)
------------------------------------------
Copy virtual path
Copy page name
Representative thumbnail: pin/unpin
------------------------------------------
Show source PDF in Explorer
Copy source PDF path
```

Future `PdfPage` addition:

- Add "Copy page image to clipboard" once a safe path exists to render or reuse
  the page bitmap off the UI thread.

`ZipDir` menu:

```text
ZIP folder/book: chapter01
------------------------------------------
Open
Copy virtual path
Representative thumbnail: pin/unpin
```

Do not include delete, rename, properties, cut, or paste for virtual items.

## 10. Interaction with fullscreen context menus

The fullscreen context menu should follow the same item classification:

- Real file pages use the native real-file menu when safe.
- `ZipImage` and `PdfPage` use the virtual custom native menu.
- Video fullscreen can expose real-file Shell menu entries, but playback input
  and native video presenter focus must be tested separately.
- Right-click-long-press behavior can remain the trigger; only the menu backend
  changes.

The fullscreen menu currently has fewer commands than the grid menu. Preserve
that if it feels better:

- Path/name copy.
- Image/frame copy.
- Representative thumbnail pin.
- Source file Shell menu or source file reveal.

## 11. Implementation phases

### Phase 0 - Windows API spike

- Add a temporary spike or small local prototype for:
  - `IFileOperation` copy-to-folder with overwrite collision.
  - `IFileOperation` delete-to-recycle with cancel.
  - `IContextMenu` for one file.
  - `IContextMenu` for multiple files in the same folder.
  - `IContextMenu2/3` message forwarding with the eframe main HWND.
- Confirm required `windows` crate feature flags. Current likely relevant
  features already include `Win32_UI_Shell`, `Win32_UI_Shell_Common`,
  `Win32_UI_WindowsAndMessaging`, `Win32_System_Com`,
  `Win32_System_Com_StructuredStorage`, `Win32_System_Memory`, and
  `Win32_System_Ole`.

### Phase 1 - Folder A/B quick workspaces

Status: implemented.

- Add reusable `FolderNavHistoryState` helpers.
- Make A active by default; B is available as a second workspace, and an
  unvisited workspace opens the drive list.
- Add A/B target persistence and session-local per-slot back/forward history.
- Add folder bar `A` / `B` buttons, active-slot highlight, and the toolbar
  preference toggle.
- Wire A/B navigation through the existing folder load and archive conversion
  paths. Do not add synchronous `read_dir` work to button handling.
- Update `folder_nav_history_snapshot` / restore helpers so search and archive
  rollback include the A/B history states.

### Phase 2 - `IFileOperation` worker

- Add `src/shell_file_ops.rs`.
- Implement direct copy/move/delete/rename requests for real paths.
- Replace `paste_files_from_clipboard` and `copy_paths_into_folder` internals.
- Re-enable folder paste/drop only through `IFileOperation`.
- Preserve the current search-view guard so paste/drop cannot target stale
  `current_folder`.
- Keep old PowerShell paths behind a temporary fallback if needed.

### Phase 3 - Real filesystem native context menu

Status: implemented for the main grid. Fullscreen can be aligned later.

- Add `src/native_context_menu.rs`.
- Route real filesystem right-clicks through native `HMENU`.
- Insert mIV commands first, then Shell commands.
- Dispatch Shell commands via `InvokeCommand`.
- Dispatch mIV commands back to `App`.
- Add an experimental setting for native Shell context menus.

### Phase 4 - Virtual native custom menu

- Replace egui context menu for `ZipImage`, `PdfPage`, `ZipDir`, and mixed
  virtual selections with native custom `HMENU`.
- Keep operations limited to mIV-safe virtual commands.
- Add PDF page image-copy only if implemented off the UI thread.

### Phase 5 - Remove duplicated egui paths

- Keep egui menu helpers as fallback only.
- Remove legacy XMP entries from context menus.
- Delete or shrink custom "Open with" enumeration if Shell menu covers the real
  file use case.

### Phase 6 - Documentation and release notes

Update, at minimum:

- `docs/spec.md`
- `docs/README.md`
- `docs/ui-responsiveness.md`
- `docs/architecture-overview.md`
- `docs/file-drag-drop-design.md` or this document, depending on final module
  names
- `htdocs/mimageviewer/manual/settings.html`
- `htdocs/mimageviewer/manual/grid.html`
- `htdocs/mimageviewer/manual/fullscreen.html`
- `htdocs/mimageviewer/manual/shortcuts.html`

## 12. Tests and validation

Automated tests:

- A/B quick folder default-active behavior, remembered target clearing,
  persistence of targets, and disabled behavior for ineligible temporary views.
- A/B history isolation: navigating inside A must not mutate B history, and
  switching A/B must not push to either history.
- A/B history snapshot/restore around search navigation and archive conversion
  cancellation/failure.
- Archive target normalization: converted archive cache paths must store and
  reload the source archive path.
- Item classification: real, virtual, mixed, search container, background.
- Native menu command ID mapping and dispatch.
- `IFileOperation` request construction for copy/move/delete/rename.
- Path normalization for `/` versus `\`, UNC, drive roots, and Japanese paths.
- Search/tag result paste guard.
- Virtual menu command availability.

Manual Windows tests:

- Copy file to folder with no conflict.
- Copy file over existing file and verify standard conflict UI appears.
- Copy folder with nested files and verify collision handling.
- Move from clipboard and verify source removal only after successful operation.
- Cancel copy/move from the Shell progress dialog.
- Delete to recycle bin.
- Delete from network/removable/no-recycle targets and verify permanent-delete
  warning.
- Rename from Shell context menu.
- Properties from Shell context menu.
- Third-party Shell extensions such as archive tools.
- Same-folder multi-selection.
- Cross-folder multi-selection fallback behavior.
- Mixed real + virtual checked selection.
- ZIP image virtual menu.
- PDF page virtual menu.
- Search result "jump to folder".
- High DPI and multi-monitor popup placement.
- Right-click after D&D and after menu cancel, checking for stuck pointer state.
- Fullscreen still image, ZIP image, PDF page, and video context menus.
- File organization smoke test: copy/cut in A, switch to B, paste through Shell
  operation, switch back to A, and verify each slot's back/forward history still
  works independently.

Performance checks:

- A/B button clicks may trigger the existing async folder load, but must not add
  synchronous folder scans, archive enumeration, or thumbnail work to the UI
  frame.
- Native menu opening should not perform file content I/O.
- `IFileOperation` can show modal Shell UI, but long copy/move/delete work must
  not run as blocking filesystem loops in `App::update`.
- If `TrackPopupMenuEx` blocks the UI thread while the menu is open, that is
  acceptable and matches native menu behavior.
- After `IFileOperation` completes, reload visible folders through existing
  async navigation/reload paths rather than synchronous `read_dir` in the UI
  frame.

## 13. Risks and mitigations

A/B state confusion:

- Risk: users may think A/B are two visible panes or expect two independent
  selections to remain active.
- Mitigation: label and tooltip them as quick folder workspaces, keep one grid,
  and make copy/cut state rely on the Shell clipboard rather than hidden mIV
  selections.

History leakage:

- Risk: A/B switching can pollute the normal folder back/forward stacks or mix
  A and B histories.
- Mitigation: introduce explicit `FolderNavHistoryState` ownership and test
  switch/navigation cases separately.

Shell extensions run in-process:

- Risk: third-party context menu handlers can hang or crash mIV.
- Mitigation: experimental setting, egui fallback, and logging around native
  menu failures.

winit/eframe window procedure handling:

- Risk: forwarding `IContextMenu2/3` messages requires temporary HWND subclassing.
- Mitigation: keep the subclass lifetime limited to the popup menu, restore
  unconditionally, and test with common owner-drawn Shell extensions.

Cancellation detection:

- Risk: treating `PerformOperations` success as full success when the user
  canceled.
- Mitigation: always query `GetAnyOperationsAborted`.

Virtual item confusion:

- Risk: users may expect ZIP entries/PDF pages to support rename/delete if the
  menu looks like Explorer.
- Mitigation: disabled header ("ZIP image", "PDF page") and no Shell destructive
  verbs for virtual items.

Reparse points and recursive folder copies:

- Risk: folder paste/drop can recurse or traverse unexpected links.
- Mitigation: delegate copy/move to Shell collision UI and avoid manual recursive
  `Copy-Item` / `std::fs` loops. Keep explicit path validation for mIV-chosen
  destinations.

## 14. Reference links

- Microsoft `IFileOperation`:
  <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation>
- Microsoft `IFileOperation::SetOperationFlags`:
  <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags>
- Microsoft `IContextMenu::QueryContextMenu`:
  <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-icontextmenu-querycontextmenu>
- Microsoft Shell shortcut menu reference:
  <https://learn.microsoft.com/en-us/windows/win32/shell/context-menu-reference>
- Microsoft `IContextMenu2::HandleMenuMsg`:
  <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-icontextmenu2-handlemenumsg>
- Raymond Chen, "How to host an IContextMenu, part 9 - Adding custom commands":
  <https://devblogs.microsoft.com/oldnewthing/20041004-00/?p=37673>
