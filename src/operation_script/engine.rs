//! The operation-script sandbox. Keep the registered surface deliberately small.

use rhai::packages::{ArithmeticPackage, BasicIteratorPackage, LogicPackage, Package};
use rhai::{Array, Dynamic, Engine, EvalAltResult, INT, Map, NativeCallContext};

use super::{MAX_COLLECTION_SIZE, MAX_OPERATIONS, MAX_STRING_BYTES};

fn is_numeric_equality_type(kind: std::any::TypeId) -> bool {
    // Rhai's built-in numeric comparison routes (func/builtin.rs:is_numeric).
    // Keep cross-type numeric operands for Rhai to compare, including int/float.
    [
        std::any::TypeId::of::<i8>(),
        std::any::TypeId::of::<u8>(),
        std::any::TypeId::of::<i16>(),
        std::any::TypeId::of::<u16>(),
        std::any::TypeId::of::<i32>(),
        std::any::TypeId::of::<u32>(),
        std::any::TypeId::of::<i64>(),
        std::any::TypeId::of::<u64>(),
        std::any::TypeId::of::<i128>(),
        std::any::TypeId::of::<u128>(),
        std::any::TypeId::of::<f32>(),
        std::any::TypeId::of::<f64>(),
    ]
    .contains(&kind)
}

/// Build the same bounded engine for worker-side compilation and UI-side calls.
pub fn build_engine() -> Engine {
    let mut engine = Engine::new_raw();
    ArithmeticPackage::new().register_into_engine(&mut engine);
    LogicPackage::new().register_into_engine(&mut engine);
    BasicIteratorPackage::new().register_into_engine(&mut engine);
    engine.register_global_module(rhai::exported_module!(safe_functions).into());
    engine.disable_symbol("eval");
    engine.disable_symbol("import");
    // These are syntactic built-ins even without BasicFnPackage. Curried
    // FnPtr values can form recursively cloned trees outside data-size limits.
    for symbol in ["Fn", "curry", "call"] {
        engine.disable_symbol(symbol);
    }
    engine.set_allow_anonymous_fn(false);
    // eval/expr.rs also resolves an otherwise missing variable matching a
    // named script function into FnPtr. Real variables/arguments still resolve
    // normally, and direct function calls never take this variable route.
    #[allow(deprecated)] // Rhai marks on_var volatile, not actually deprecated.
    engine.on_var(|name, index, context| {
        if index == 0 && !context.scope().contains(name) {
            Err(EvalAltResult::ErrorVariableNotFound(name.into(), rhai::Position::NONE).into())
        } else {
            Ok(None)
        }
    });
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(32, 32);
    engine.set_max_string_size(MAX_STRING_BYTES);
    engine.set_max_array_size(MAX_COLLECTION_SIZE);
    engine.set_max_map_size(MAX_COLLECTION_SIZE);
    engine
}

// Do not add a standard string/array/map package here. Each of these functions
// terminates after traversing bounded input; none performs I/O or calls scripts.
#[rhai::export_module]
mod safe_functions {
    use super::*;

    #[rhai_fn(name = "contains")]
    pub fn string_contains(string: &str, substring: &str) -> bool {
        string.contains(substring)
    }

    #[rhai_fn(name = "starts_with")]
    pub fn string_starts_with(string: &str, prefix: &str) -> bool {
        string.starts_with(prefix)
    }

    #[rhai_fn(name = "ends_with")]
    pub fn string_ends_with(string: &str, suffix: &str) -> bool {
        string.ends_with(suffix)
    }

    #[rhai_fn(name = "to_lower")]
    pub fn string_to_lower(string: &str) -> String {
        // Unicode case expansion is bounded by the input. Rhai checks the
        // returned string against max_string_size after the native call.
        string.to_lowercase()
    }

    #[rhai_fn(name = "len")]
    pub fn string_len(string: &str) -> INT {
        string.chars().count() as INT
    }

    #[rhai_fn(name = "len", pure)]
    pub fn array_len(array: &mut Array) -> INT {
        array.len() as INT
    }

    #[rhai_fn(name = "contains", pure)]
    pub fn map_contains(map: &mut Map, property: &str) -> bool {
        map.contains_key(property)
    }

    /// Match Rhai's array membership semantics, using native equality only.
    #[rhai_fn(name = "contains", return_raw, pure)]
    pub fn array_contains(
        ctx: NativeCallContext,
        array: &mut Array,
        value: Dynamic,
    ) -> Result<bool, Box<EvalAltResult>> {
        let value_kind = value.type_id();
        let value_is_numeric = is_numeric_equality_type(value_kind);
        let mut value = value;
        for item in array {
            let item_kind = item.type_id();
            if item_kind != value_kind
                && !(value_is_numeric && is_numeric_equality_type(item_kind))
                && !(value.is_char() && item.is_string())
                && !(value.is_string() && item.is_char())
            {
                // Other unlike types compare false in this engine. Do not
                // copy a nested array/map merely to discover that mismatch.
                continue;
            }
            // Every native == exposed by this sandbox is symmetric and does
            // not mutate either operand (builtin.rs and LogicPackage). Keep
            // the needle in the non-consumed first position, and copy each
            // element at most once. Copying the needle per element would turn
            // a nested needle into O(array length * needle size) work.
            // Array/Map == are not registered: same-type composite operands
            // retain Rhai's ErrorFunctionNotFound instead of gaining equality.
            let equal = ctx
                .call_native_fn_raw("==", true, &mut [&mut value, &mut item.clone()])
                .or_else(|error| match *error {
                    EvalAltResult::ErrorFunctionNotFound(ref signature, ..)
                        if signature.starts_with("==") && item_kind != value_kind =>
                    {
                        Ok(Dynamic::FALSE)
                    }
                    _ => Err(error),
                })?;
            if equal.as_bool().unwrap_or(false) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhai::Scope;

    #[test]
    fn sandbox_exposes_exactly_the_audited_signatures() {
        let mut actual = build_engine().gen_fn_signatures(true);
        actual.sort();
        let expected: Vec<_> = include_str!("allowed-signatures.txt")
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            actual, expected,
            "Audit source changes before changing the allowlist"
        );
        for forbidden in ["sleep", "pad", "print", "debug"] {
            assert!(
                actual
                    .iter()
                    .all(|signature| !signature.starts_with(&format!("{forbidden}(")))
            );
        }
    }

    #[test]
    fn sandbox_rejects_eval_and_import_during_compilation() {
        let engine = build_engine();
        for script in [r#"eval("1")"#, r#"import "anything" as module;"#] {
            assert!(engine.compile(script).is_err(), "{script}");
        }
    }

    #[test]
    fn sandbox_rejects_function_pointer_construction_curry_call_and_closures() {
        let engine = build_engine();
        assert!(!engine.allow_anonymous_fn());
        for script in [
            r#"Fn("x")"#,
            r#"fn x() {} fn slots() { let f = Fn("x"); loop { f = f.curry(f); } }"#,
            "let f = 0; f.curry(f)",
            "let f = 0; curry(f, f)",
            "let f = 0; f.call()",
            "let f = 0; call(f)",
            "let f = || 1;",
            "let f = |x| x + 1;",
            "let captured = [1]; let f = || captured;",
        ] {
            assert!(engine.compile(script).is_err(), "{script}");
        }
    }

    #[test]
    fn sandbox_rejects_named_function_values_but_keeps_direct_calls_and_variables() {
        let engine = build_engine();
        for script in [
            "fn x() {} let f = x; f",
            "fn x() {} fn slots() { x } slots()",
            "fn x() {} const f = x; f",
            "fn x() {} [x]",
            "fn x() {} #{ callback: x }",
        ] {
            assert!(engine.run(script).is_err(), "{script}");
        }
        for script in [
            "fn add(x) { x + 1 } add(41)",
            "fn add(x) { x + 1 } fn run(x) { add(x) } run(41)",
            "fn add(x) { x + 1 } let add = 42; add",
            "const n = 41; fn add(x) { global::n + x } add(1)",
        ] {
            assert_eq!(engine.eval::<INT>(script).unwrap(), 42, "{script}");
        }
        let mut scope = Scope::new();
        scope.push("external", 42 as INT);
        assert_eq!(
            engine
                .eval_with_scope::<INT>(&mut scope, "external")
                .unwrap(),
            42
        );
    }

    #[test]
    fn sandbox_has_no_sleep_pad_print_debug_or_external_helpers() {
        let engine = build_engine();
        for script in [
            "sleep(0)",
            "sleep(0.0)",
            r#""".pad(1, "")"#,
            r#"pad("", 1, "")"#,
            r#"print("private")"#,
            r#"debug("private")"#,
            r#"regex_is_match("a", "a")"#,
            r#"read_file("private")"#,
            "timestamp()",
            "[1, 2].sort()",
        ] {
            assert!(
                matches!(
                    *engine.run(script).unwrap_err(),
                    EvalAltResult::ErrorFunctionNotFound(_, _)
                ),
                "{script}"
            );
        }
    }

    #[test]
    fn sandbox_supports_conditions_unicode_and_membership() {
        let engine = build_engine();
        for script in [
            r#""MANGA/あ.PNG".to_lower() == "manga/あ.png""#,
            r#""あいう".len() == 3"#,
            r#""manga/あ.png".contains("/あ")"#,
            r#""manga/あ.png".starts_with("manga/")"#,
            r#""manga/あ.png".ends_with(".png")"#,
            r#""page" in ["view", "page"]"#,
            r#"3 in [1, "two", 3]"#,
            r#"!(4 in [1, "two", 3])"#,
            r#""context" in #{ context: "grid" }"#,
            r#"!("missing" in #{ context: "grid" })"#,
            "[1, 2, 3].len() == 3",
            "let total = 0; for n in 0..4 { total += n; } total == 6",
        ] {
            assert!(engine.eval::<bool>(script).unwrap(), "{script}");
        }
    }

    #[test]
    fn sandbox_array_membership_matches_native_equality_in_both_orders() {
        let engine = build_engine();
        for (left, right) in [
            ("1", "1.0"),
            ("1.0", "1"),
            ("1", "1.0000000000000002"),
            ("1.0000000000000002", "1"),
            ("'あ'", r#""あ""#),
            (r#""あ""#, "'あ'"),
            ("'a'", r#""ab""#),
            (r#""ab""#, "'a'"),
            ("()", "()"),
            ("true", "false"),
            ("1..3", "1..3"),
            ("1..=3", "1..=3"),
            ("1..3", "1..=3"),
        ] {
            let expected = engine
                .eval::<bool>(format!("({left}) == ({right})"))
                .unwrap();
            let actual = engine
                .eval::<bool>(format!("({left}) in [({right})]"))
                .unwrap();
            assert_eq!(actual, expected, "{left} in [{right}]");
        }
    }

    #[test]
    fn sandbox_array_membership_skips_unlike_nested_values_and_preserves_elements() {
        let engine = build_engine();
        for source in [
            "!([1, 2] in [1, true, (), #{ a: 1 }])",
            "!(#{ a: [1, 2] } in [1, true, (), [1, 2]])",
            "let a = [[1, 2], #{ a: [3] }, 4]; let found = 4 in a; found && a[0][1] == 2 && a[1].a[0] == 3 && a[2] == 4",
            r#"let a = [1.0, "あ", 2]; let first = 1 in a; let second = 'あ' in a; first && second && a[0] == 1.0 && a[1] == "あ" && a[2] == 2"#,
        ] {
            assert!(engine.eval::<bool>(source).unwrap(), "{source}");
        }
    }

    #[test]
    fn sandbox_array_membership_keeps_same_type_composite_equality_unavailable() {
        let engine = build_engine();
        // Equality for arrays/maps belongs to packages deliberately absent
        // from this engine. This is the pre-existing native failure contract.
        for source in [
            "[1, [2]] in [[1, [2]]]",
            "#{ a: [1] } in [#{ a: [1] }]",
            "[1] in [0, [1]]",
            "#{ a: 1 } in [0, #{ a: 1 }]",
        ] {
            assert!(
                matches!(
                    *engine.eval::<bool>(source).unwrap_err(),
                    EvalAltResult::ErrorFunctionNotFound(ref signature, _)
                        if signature.starts_with("==")
                ),
                "{source}"
            );
        }
    }

    #[test]
    fn sandbox_array_membership_clones_nested_needles_only_linearly() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct CountedClone(Arc<AtomicUsize>);
        impl Clone for CountedClone {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, Ordering::Relaxed);
                Self(Arc::clone(&self.0))
            }
        }
        let engine = build_engine();
        let clones = Arc::new(AtomicUsize::new(0));
        let mut scope = Scope::new();
        scope.push("values", vec![Dynamic::from(0 as INT); MAX_COLLECTION_SIZE]);
        let nested: Array = (0..MAX_COLLECTION_SIZE)
            .map(|_| Dynamic::from(CountedClone(Arc::clone(&clones))))
            .collect();
        scope.push("needle", nested);
        clones.store(0, Ordering::Relaxed);
        assert!(
            !engine
                .eval_with_scope::<bool>(&mut scope, "values.contains(needle)")
                .unwrap()
        );
        assert!(clones.load(Ordering::Relaxed) <= MAX_COLLECTION_SIZE * 8);

        let nested: Map = (0..MAX_COLLECTION_SIZE)
            .map(|n| {
                (
                    format!("k{n}").into(),
                    Dynamic::from(CountedClone(Arc::clone(&clones))),
                )
            })
            .collect();
        scope.set_value("needle", nested);
        clones.store(0, Ordering::Relaxed);
        assert!(
            !engine
                .eval_with_scope::<bool>(&mut scope, "values.contains(needle)")
                .unwrap()
        );
        assert!(clones.load(Ordering::Relaxed) <= MAX_COLLECTION_SIZE * 8);
    }

    #[test]
    fn sandbox_operation_budget_stops_infinite_loops() {
        assert!(matches!(
            *build_engine().run("loop {}").unwrap_err(),
            EvalAltResult::ErrorTooManyOperations(_)
        ));
    }

    #[test]
    fn sandbox_limits_call_and_expression_depth() {
        let engine = build_engine();
        let error = engine
            .run("fn recurse() { recurse() } recurse()")
            .unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("stack overflow"),
            "{error}"
        );
        let nested = format!("{}true{}", "!(".repeat(40), ")".repeat(40));
        assert!(engine.compile(&nested).is_err());
        assert!(engine.compile(format!("fn deep() {{ {nested} }}")).is_err());
    }

    #[test]
    fn sandbox_enforces_string_array_and_map_sizes() {
        let engine = build_engine();
        assert_eq!(engine.max_operations(), MAX_OPERATIONS);
        assert_eq!(engine.max_call_levels(), 32);
        assert_eq!(engine.max_expr_depth(), 32);
        assert_eq!(engine.max_function_expr_depth(), 32);
        assert_eq!(engine.max_string_size(), MAX_STRING_BYTES);
        assert_eq!(engine.max_array_size(), MAX_COLLECTION_SIZE);
        assert_eq!(engine.max_map_size(), MAX_COLLECTION_SIZE);
        assert!(
            engine
                .run(format!("\"{}\";", "a".repeat(MAX_STRING_BYTES)))
                .is_ok()
        );
        assert!(
            engine
                .run(format!("\"{}\";", "a".repeat(MAX_STRING_BYTES + 1)))
                .is_err()
        );
        // Native Unicode case conversion can expand U+0130 into two codepoints.
        let expanded = format!("\"{}\".to_lower();", "İ".repeat(MAX_STRING_BYTES / 2));
        assert!(engine.run(expanded).is_err());
        for (count, succeeds) in [
            (MAX_COLLECTION_SIZE, true),
            (MAX_COLLECTION_SIZE + 1, false),
        ] {
            let array = format!("[{}];", vec!["0"; count].join(","));
            let map = format!(
                "#{{{}}};",
                (0..count)
                    .map(|n| format!("k{n}: 0"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            assert_eq!(engine.run(array).is_ok(), succeeds, "array {count}");
            assert_eq!(engine.run(map).is_ok(), succeeds, "map {count}");
        }
    }

    #[test]
    fn sandbox_checks_runtime_string_and_map_growth() {
        let engine = build_engine();
        let mut scope = Scope::new();
        scope.push("text", "a".repeat(MAX_STRING_BYTES));
        assert!(matches!(
            *engine
                .run_with_scope(&mut scope, r#"text += "b";"#)
                .unwrap_err(),
            EvalAltResult::ErrorDataTooLarge(_, _)
        ));
        let values: Map = (0..MAX_COLLECTION_SIZE)
            .map(|n| (format!("k{n}").into(), Dynamic::UNIT))
            .collect();
        scope.push("values", values);
        assert!(matches!(
            *engine
                .run_with_scope(&mut scope, "values.extra = (); ")
                .unwrap_err(),
            EvalAltResult::ErrorDataTooLarge(_, _)
        ));
    }

    fn measure_to_operation_limit(name: &str, scope: &mut Scope<'_>, source: &str) {
        assert!(
            !cfg!(debug_assertions),
            "Run ignored measurements with --release"
        );
        let engine = build_engine();
        let ast = engine.compile(source).unwrap();
        let start = std::time::Instant::now();
        let error = engine.run_ast_with_scope(scope, &ast).unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            matches!(*error, EvalAltResult::ErrorTooManyOperations(_)),
            "{error}"
        );
        eprintln!(
            "[operation-script worst case] {name}: {elapsed:?}, max_operations={MAX_OPERATIONS}"
        );
    }

    #[test]
    #[ignore = "release-only worst-case measurement; run with --release --ignored --nocapture"]
    fn perf_max_string_contains_to_operation_limit() {
        let mut scope = Scope::new();
        scope.push("text", "a".repeat(MAX_STRING_BYTES));
        scope.push("needle", format!("{}b", "a".repeat(MAX_STRING_BYTES - 1)));
        measure_to_operation_limit(
            "string contains",
            &mut scope,
            "loop { text.contains(needle); }",
        );
    }

    #[test]
    #[ignore = "release-only worst-case measurement; run with --release --ignored --nocapture"]
    fn perf_max_array_contains_to_operation_limit() {
        let mut scope = Scope::new();
        // Rhai limits aggregate string bytes within a collection, so fill the
        // array with integers and force a complete unsuccessful traversal.
        scope.push(
            "values",
            (0..MAX_COLLECTION_SIZE)
                .map(|n| Dynamic::from(n as INT))
                .collect::<Array>(),
        );
        measure_to_operation_limit(
            "array contains",
            &mut scope,
            "loop { values.contains(-1); }",
        );
    }

    #[test]
    #[ignore = "release-only worst-case measurement; run with --release --ignored --nocapture"]
    fn perf_max_array_contains_with_composite_needle_to_operation_limit() {
        let mut scope = Scope::new();
        let values: Array = (0..MAX_COLLECTION_SIZE)
            .map(|n| Dynamic::from(n as INT))
            .collect();
        let nested: Array = vec![Dynamic::from(0 as INT); MAX_COLLECTION_SIZE];
        scope.push("values", values);
        scope.push("needle", nested);
        measure_to_operation_limit(
            "array contains with max-size unlike nested needle",
            &mut scope,
            "loop { values.contains(needle); }",
        );

        let nested: Map = (0..MAX_COLLECTION_SIZE)
            .map(|n| (format!("k{n}").into(), Dynamic::UNIT))
            .collect();
        scope.set_value("needle", nested);
        measure_to_operation_limit(
            "array contains with max-size unlike map needle",
            &mut scope,
            "loop { values.contains(needle); }",
        );
    }

    #[test]
    #[ignore = "release-only worst-case measurement; run with --release --ignored --nocapture"]
    fn perf_max_map_contains_to_operation_limit() {
        let mut scope = Scope::new();
        let key_prefix = "a".repeat(MAX_STRING_BYTES - 4);
        let values: Map = (0..MAX_COLLECTION_SIZE)
            .map(|n| (format!("{key_prefix}{n:04}").into(), Dynamic::UNIT))
            .collect();
        scope.push("values", values);
        scope.push("needle", format!("{key_prefix}9999"));
        measure_to_operation_limit(
            "map contains",
            &mut scope,
            "loop { values.contains(needle); }",
        );
    }

    #[test]
    #[ignore = "release-only measurement of the unresolved opaque iterator size boundary"]
    fn perf_max_chars_stream_collection_clone_to_operation_limit() {
        // BasicIteratorPackage exposes a CharsStream whose stored characters
        // are not included in Rhai's string/collection size accounting.
        // Keep the package required by the design; measure this boundary for
        // the design owner before connecting UI-side evaluation in stage 2.
        let engine = build_engine();
        let mut scope = Scope::new();
        scope.push("text", "a".repeat(MAX_STRING_BYTES));
        let stream = engine
            .eval_with_scope::<Dynamic>(&mut scope, "text.chars()")
            .unwrap();
        scope.push("values", vec![stream; MAX_COLLECTION_SIZE]);
        measure_to_operation_limit(
            "opaque chars stream collection clone",
            &mut scope,
            "loop { let copy = values; }",
        );
    }
}
