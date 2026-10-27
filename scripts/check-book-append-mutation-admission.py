"""Audit the three App media-metadata write entrances (stdlib only).

Run: python scripts/check-book-append-mutation-admission.py
This is a bounded lexical source audit, not Rust type/control-flow analysis.
It strips comments/literals and cfg(test) items, checks the fixed user-writer
inventory, and rejects unclassified writes in functions referencing these App
stores. Renames/new writers require an explicit inventory update. DB-internal
SQL, macros, interprocedural aliases and unrelated settings are outside scope.
"""

from dataclasses import dataclass
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
ACCESSOR_FILE = "src/app/media_metadata_mutation.rs"
GATE = "admit_book_append_followup_mutation"
STORES = {
    "folder_thumb_pin_db": ("user_folder_thumb_pin_db", {"set", "remove"}),
    "video_bookmark_db": (
        "user_video_bookmark_db",
        {"add", "add_if_no_duplicate", "bulk_add_if_no_duplicate", "remove",
         "update_title", "update_thumb", "clear_for"},
    ),
    "video_pin_db": ("user_video_pin_db", {"set_pin", "remove"}),
}

# (file, function): (store, first DB mutation). Wrapper/UI routes converge here.
USER_WRITERS = {
    ("src/app.rs", "set_folder_thumb_pin"): ("folder_thumb_pin_db", "set"),
    ("src/app.rs", "remove_folder_thumb_pin"): ("folder_thumb_pin_db", "remove"),
    ("src/app/native_video.rs", "add_native_video_bookmark"):
        ("video_bookmark_db", "add"),
    ("src/app/native_video.rs", "handle_native_video_delete_bookmark_command"):
        ("video_bookmark_db", "remove"),
    ("src/app/native_video.rs", "handle_native_video_set_bookmark_title_command"):
        ("video_bookmark_db", "update_title"),
    ("src/app/native_video.rs", "handle_native_video_bulk_add_bookmarks_command"):
        ("video_bookmark_db", "bulk_add_if_no_duplicate"),
    ("src/app/native_video.rs", "handle_native_video_clear_all_bookmarks_command"):
        ("video_bookmark_db", "clear_for"),
    ("src/app/native_video.rs", "set_native_video_pin_with_webp"):
        ("video_pin_db", "set_pin"),
    ("src/app/native_video.rs", "handle_native_video_delete_pin_command"):
        ("video_pin_db", "remove"),
    ("src/ui_music_panels.rs", "add_music_bookmark_at"):
        ("video_bookmark_db", "add_if_no_duplicate"),
    ("src/ui_music_panels.rs", "delete_music_bookmark"):
        ("video_bookmark_db", "remove"),
    ("src/ui_music_panels.rs", "rename_music_bookmark"):
        ("video_bookmark_db", "update_title"),
    ("src/ui_music_panels.rs", "bulk_add_music_bookmarks"):
        ("video_bookmark_db", "bulk_add_if_no_duplicate"),
    ("src/ui_music_panels.rs", "clear_all_music_bookmarks"):
        ("video_bookmark_db", "clear_for"),
    ("src/ui_fullscreen.rs", "handle_video_input"):
        ("video_bookmark_db", "add"),
}

# These complete/regenerate derived thumbnails, not a new accepted user edit.
# Keep exact method sets so the allowlist cannot hide new title/remove writers.
MAINTENANCE = {
    ("src/app/native_video.rs", "persist_native_video_marker_thumbnail"):
        {("video_pin_db", "set_pin"), ("video_bookmark_db", "update_thumb")},
    ("src/app/native_video.rs", "tick_pending_pin_thumb_refresh"):
        {("video_pin_db", "set_pin")},
}

# Other homologous user writers found during LISTrev8. Keep the first mutation
# token explicit so admission cannot silently drift to the completion side.
DIRECT_WRITERS = {
    ("src/app/book_composition_runtime.rs", "persist_fullscreen_view_mode"):
        "service.set_view_mode(",
    ("src/app/book_composition_runtime.rs", "persist_fullscreen_reading_flow"):
        "service.set_view_flow(",
    ("src/app/page_edit_owner.rs", "set_local_adjust_layers_for_target"):
        "self.set_local_adjust_layers_for_target_memory_only(",
    ("src/app/book_composition_editor.rs", "save_local_composition_editor"):
        "self.composition_editor_operations.tickets.push(",
    ("src/app/image_folder_zip_operation.rs", "start_image_zip_worker"):
        "app.take_pending_view_trim_transfer(",
    ("src/app.rs", "delete_bookmark_browser_rows"): "crate::bookmark_browser::spawn_delete(",
    ("src/app.rs", "remove_reading_history_entry_for_idx"): ".remove_key(",
    ("src/app.rs", "remove_favorite_view_state"): "self.favorite_view_writes.remove(",
    ("src/app.rs", "clear_all_favorite_view_states"): "self.favorite_view_writes.clear(",
    ("src/ui_erase.rs", "save_mask_to_slot"): ".set_slot(",
    ("src/ui_conceal.rs", "save_conceal_mask_to_slot"): ".set_slot(",
    ("src/ui_dialogs/rotation_reset.rs", "reset_saved_rotations"): ".clear_all(",
    ("src/app/book_resume_meter.rs", "clear_book_resume_async"): "writer.clear(",
    ("src/app/content_identity_restore.rs", "start_content_identity_restore"):
        "ContentIdentityRestorePending::spawn(",
    ("src/app/content_identity_restore.rs", "apply_content_restore_disable_choice_to_runtime"):
        "apply_disable_choice_to_settings(",
    ("src/ui_dialogs/settings_restore.rs", "execute_pending_settings_restore"):
        "self.settings_restore_state.pending = None",
    ("src/ui_dialogs/favorites_editor.rs", "show_favorites_editor_dialog"):
        "self.settings.favorites.remove(",
}

# Service-specific methods: enumerate every production caller, including the
# read/adoption transitions which only flush already accepted trim edits.
COMPOSITION_VIEW_WRITERS = {
    ("src/app/book_composition_runtime.rs", "persist_fullscreen_view_mode"): {"set_view_mode"},
    ("src/app/book_composition_runtime.rs", "persist_fullscreen_reading_flow"): {"set_view_flow"},
    ("src/app.rs", "persist_current_view_trim_book_state"): {"save_view_trim"},
    ("src/app.rs", "persist_view_trim_page_override"): {"save_view_trim"},
    ("src/app.rs", "remove_view_trim_page_override"): {"remove_view_trim_page"},
    ("src/remote_ipc/ui.rs", "apply_pending_remote_write"): {"set_view_mode", "set_view_trim_book"},
}
COMPOSITION_VIEW_MAINTENANCE = {
    ("src/app.rs", "apply_view_trim_for_key_with_fallback"): {"save_view_trim"},
    ("src/app/book_composition_runtime.rs", "begin_book_projection"): {"save_view_trim"},
}
COMPOSITION_VIEW_METHODS = {
    method for inventory in (COMPOSITION_VIEW_WRITERS, COMPOSITION_VIEW_MAINTENANCE)
    for methods in inventory.values() for method in methods
}

FN = re.compile(r"\bfn\s+(\w+)\s*(?:<[^;{}]*>)?\s*\(")
CALL = re.compile(r"\b(\w+)\s*\.\s*(\w+)\s*\(")


@dataclass
class Function:
    path: str
    name: str
    line: int
    body: str

    @property
    def key(self):
        return self.path, self.name

    @property
    def label(self):
        return f"{self.path}:{self.line}::{self.name}"


def blank(text):
    return "".join("\n" if c == "\n" else " " for c in text)


def mask_noncode(text):
    """Preserve positions while masking Rust comments, strings and char literals."""
    token = re.compile(
        r'//[^\n]*|/\*|\b(?:br|r)(#+)"|\b(?:br|r)"|"|'
        r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'"
    )
    chunks = []
    pos = 0
    while match := token.search(text, pos):
        chunks.append(text[pos:match.start()])
        start = match.start()
        kind = match.group()
        end = match.end()
        if kind == "/*":
            depth = 1
            while depth and end < len(text):
                nested = re.search(r"/\*|\*/", text[end:])
                if nested is None:
                    end = len(text)
                    break
                depth += 1 if nested.group() == "/*" else -1
                end += nested.end()
        elif kind.startswith(("r", "br")):
            terminator = '"' + (match.group(1) or "")
            closing = text.find(terminator, end)
            end = len(text) if closing < 0 else closing + len(terminator)
        elif kind == '"':
            closing = re.search(r'(?:\\.|[^"\\])*"', text[end:])
            end = len(text) if closing is None else end + closing.end()
        chunks.append(blank(text[start:end]))
        pos = end
    chunks.append(text[pos:])
    return "".join(chunks)


def block_end(text, opening):
    depth = 1
    for match in re.finditer(r"[{}]", text[opening + 1:]):
        depth += 1 if match.group() == "{" else -1
        if depth == 0:
            return opening + 1 + match.end()
    raise ValueError("unbalanced Rust item braces")


def mask_test_items(text):
    # Strings are already masked: feature="test-script" is not cfg(test).
    cfg = re.compile(r"#\[\s*(?:cfg\s*\(([^\]]*)\)|test)\s*\]")
    spans = []
    for match in cfg.finditer(text):
        condition = match.group(1)
        # Keep any(windows, test): that item also exists in production.
        if condition is not None and not requires_test(condition.strip()):
            continue
        tail = text[match.end():]
        item = re.match(
            r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?"
            r"(?:(?:unsafe|async|const)\s+)*(?:fn|mod|impl|struct|enum|use|static)\b",
            tail,
        )
        if item is None:
            continue  # Test-only struct fields do not contain App write functions.
        start = match.end() + item.end()
        opening = re.search(r"[;{]", text[start:])
        if opening:
            at = start + opening.start()
            end = at + 1 if opening.group() == ";" else block_end(text, at)
            spans.append((match.start(), end))
    for start, end in reversed(spans):
        text = text[:start] + blank(text[start:end]) + text[end:]
    return text


def requires_test(condition):
    """Only exclude cfg branches whose truth requires the test flag."""
    if condition == "test":
        return True
    match = re.fullmatch(r"(all|any)\s*\((.*)\)", condition, re.S)
    if not match:
        return False
    parts, start, depth = [], 0, 0
    value = match.group(2)
    for index, char in enumerate(value):
        depth += (char == "(") - (char == ")")
        if char == "," and depth == 0:
            parts.append(value[start:index].strip())
            start = index + 1
    parts.append(value[start:].strip())
    answers = [requires_test(part) for part in parts if part]
    return bool(answers) and (any(answers) if match.group(1) == "all" else all(answers))


def functions(path, source):
    code = mask_test_items(mask_noncode(source))
    result = []
    for match in FN.finditer(code):
        opening = re.search(r"[;{]", code[match.end():])
        if opening is None or opening.group() == ";":
            continue
        at = match.end() + opening.start()
        end = block_end(code, at)
        result.append(Function(path, match.group(1),
                               code.count("\n", 0, match.start()) + 1,
                               code[at + 1:end - 1]))
    return result


def store_writes(function):
    """Find lexical db/alias writes where the function references an App store.

    This deliberately errs toward reporting an unclassified function; the fixed
    user inventory and exact maintenance allowlist resolve ambiguous db aliases.
    Local collection methods on self/map are not DB writes.
    """
    found = []
    for field, (accessor, methods) in STORES.items():
        if not re.search(rf"\bself\s*\.\s*(?:{field}|{accessor})\b", function.body):
            continue
        aliases = {"db", field}
        # Recognize common let Some(store) / let store = self.field borrows.
        for binding in re.finditer(
            rf"(?:let|if\s+let)\s+(?:Some\(\s*)?(\w+)\s*\)?\s*=\s*"
            rf"(?:&\s*(?:mut\s+)?)?self\s*\.\s*(?:{field}|{accessor})\b",
            function.body,
        ):
            aliases.add(binding.group(1))
        for call in CALL.finditer(function.body):
            receiver, method = call.groups()
            if method in methods and (receiver in aliases or receiver.endswith("_db")):
                found.append((field, method, call.start()))
        # Chained direct writes such as self.video_pin_db.as_ref().unwrap().remove(...).
        chain = re.compile(
            rf"\bself\s*\.\s*{field}\b(?:\s*\.\s*"
            rf"(?:as_ref|as_mut|as_deref|unwrap|expect)\s*\([^;{{}}]*?\))*"
            rf"\s*\.\s*({'|'.join(sorted(methods))})\s*\("
        )
        for call in chain.finditer(function.body):
            found.append((field, call.group(1), call.start()))
    return sorted(set(found))


def audit(root):
    errors = []
    all_functions = []
    file_count = 0
    for file in sorted((root / "src").rglob("*.rs")):
        if "tests" in file.parts or file.stem == "tests" or file.stem.endswith("_tests"):
            continue
        path = file.relative_to(root).as_posix()
        try:
            all_functions.extend(functions(path, file.read_text(encoding="utf-8")))
        except ValueError as error:
            errors.append(f"{path}: {error}")
        file_count += 1
    by_key = {}
    for function in all_functions:
        by_key.setdefault(function.key, []).append(function)

    def require(key):
        candidates = by_key.get(key, [])
        if len(candidates) != 1:
            errors.append(f"{key}: expected one production function, got {len(candidates)}")
            return None
        return candidates[0]

    for field, (accessor, _) in STORES.items():
        function = require((ACCESSOR_FILE, accessor))
        if function is None:
            continue
        existence = re.search(rf"self\.{field}\.as_ref\(\)\?\s*;", function.body)
        guard = re.search(
            rf"if\s+!self\.{GATE}\(\)\s*\{{\s*return\s+None\s*;\s*\}}",
            function.body,
        )
        returns = list(re.finditer(rf"self\.{field}\.as_(?:ref|mut|deref)\(\)", function.body))
        if not existence or not guard or len(returns) != 2 or not (
            existence.end() <= guard.start() < guard.end() <= returns[-1].start()
        ):
            errors.append(f"{function.label}: expected missing-DB check -> rejecting gate -> borrow")

    for key, (field, first_write) in USER_WRITERS.items():
        function = require(key)
        if function is None:
            continue
        accessor = STORES[field][0]
        admission = re.search(
            rf"let\s+Some\(db\)\s*=\s*self\.{accessor}\(\)\s*else"
            rf"\s*\{{\s*return(?:\s+false)?\s*;\s*\}}\s*;", function.body,
        )
        writes = store_writes(function)
        matching = [position for store, method, position in writes
                    if store == field and method == first_write]
        if not admission or not matching or admission.end() > min(matching):
            errors.append(f"{function.label}: expected rejecting {accessor} before db.{first_write}")
        if admission and any(position < admission.end() for _, _, position in writes):
            errors.append(f"{function.label}: DB write precedes admission")
        # A direct field-derived db alias alongside the accessor is a bypass.
        direct = re.search(rf"(?:let|if\s+let)\b[^;]*=\s*self\.{field}\.as_", function.body)
        if direct:
            errors.append(f"{function.label}: direct store borrow bypasses the user accessor")

    for key, expected in MAINTENANCE.items():
        function = require(key)
        if function is None:
            continue
        actual = {(store, method) for store, method, _ in store_writes(function)}
        if actual != expected:
            errors.append(f"{function.label}: maintenance writes changed: {sorted(actual)}")
        if GATE in function.body or any(a in function.body for a, _ in STORES.values()):
            errors.append(f"{function.label}: automatic thumbnail maintenance must not admit a user edit")

    for key, mutation in DIRECT_WRITERS.items():
        function = require(key)
        if function is None:
            continue
        compact = re.sub(r"\s+", "", function.body)
        guard = compact.find(f"self.{GATE}()")
        first = compact.find(re.sub(r"\s+", "", mutation))
        if guard < 0 or first < 0 or guard >= first:
            errors.append(f"{function.label}: admission must precede {mutation}")

    composition_calls = re.compile(
        r"\.\s*(" + "|".join(sorted(COMPOSITION_VIEW_METHODS)) + r")\s*\("
    )
    for key, methods in (COMPOSITION_VIEW_WRITERS | COMPOSITION_VIEW_MAINTENANCE).items():
        function = require(key)
        if function is None:
            continue
        calls = list(composition_calls.finditer(function.body))
        actual = {call.group(1) for call in calls}
        if actual != methods:
            errors.append(f"{function.label}: composition view writes changed: {sorted(actual)}")
        if key in COMPOSITION_VIEW_WRITERS:
            guard = re.search(rf"if\s+!self\.{GATE}\(\)", function.body)
            if not guard or any(call.start() < guard.end() for call in calls):
                errors.append(f"{function.label}: composition view enqueue must follow admission")
        elif GATE in function.body:
            errors.append(f"{function.label}: automatic composition transition must not admit an edit")
    for function in all_functions:
        if composition_calls.search(function.body) and function.key not in (
            COMPOSITION_VIEW_WRITERS | COMPOSITION_VIEW_MAINTENANCE
        ):
            errors.append(f"{function.label}: unclassified composition view writer")

    preferences = require(("src/ui_dialogs/preferences.rs", "show_preferences_dialog"))
    if preferences is not None and not re.search(
        rf"if\s+clear_reading_history_requested\s*&&\s*self\.{GATE}\(\)", preferences.body
    ):
        errors.append(f"{preferences.label}: reading-history clear must use the common admission")

    for function in all_functions:
        writes = store_writes(function)
        if writes and function.key not in USER_WRITERS and function.key not in MAINTENANCE:
            errors.append(f"{function.label}: unclassified media-store writer "
                          f"{sorted({(s, m) for s, m, _ in writes})}")
        accessors = [a for a, _ in STORES.values() if re.search(rf"self\.{a}\(\)", function.body)]
        if accessors and function.key not in USER_WRITERS:
            errors.append(f"{function.label}: unclassified user accessor caller {accessors}")
    return errors, file_count, len(all_functions)


def main():
    errors, files, count = audit(ROOT)
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    if errors:
        print(f"FAIL: {len(errors)} findings; {files} Rust files / {count} functions scanned")
        return 1
    print(f"PASS: {len(USER_WRITERS)} media writers, {len(DIRECT_WRITERS) + 1} other user writers, "
          f"{len(STORES)} admission accessors, "
          f"{len(MAINTENANCE)} automatic maintenance exclusions; "
          f"{len(COMPOSITION_VIEW_WRITERS)} composition view writers, "
          f"{len(COMPOSITION_VIEW_MAINTENANCE)} automatic composition transitions; "
          f"{files} Rust files / {count} functions scanned")
    return 0


if __name__ == "__main__":
    sys.exit(main())
