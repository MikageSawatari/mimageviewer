//! Reject container mutations before any script code (including slots()) runs.

use rhai::{AST, ASTNode, Engine, Expr, Position, Stmt};

/// The worker's only compilation path. Optimization must remain disabled so
/// every assignment is still present when AST::walk visits function bodies.
pub(super) fn compile_checked(engine: &Engine, source: &str) -> Result<AST, String> {
    let ast = engine
        .compile(source)
        .map_err(|error| format!("コンパイルに失敗: {error}"))?;
    let mut failure = None;
    ast.walk(&mut |path| {
        match path.last() {
            Some(ASTNode::Stmt(Stmt::Assignment(assignment))) => {
                // The parser uses the same Assignment node for = and every op-assignment.
                // A chain's root is Index/Dot even for nested/mixed access paths.
                // ThisPtr is also forbidden: a[0].method() can bind `this` directly to
                // an element, so assigning `this` would mutate the parent indirectly.
                if !matches!(&assignment.1.lhs, Expr::Variable(..)) {
                    failure = Some(compile_error(
                        assignment.1.lhs.start_position(),
                        "添字・プロパティへの代入は使えません。thisへの代入も含め、変数そのものへの代入だけが使用できます",
                    ));
                    return false;
                }
            }
            Some(ASTNode::Expr(expression))
                if matches!(expression, Expr::Array(..) | Expr::Map(..) | Expr::InterpolatedString(..)) =>
            {
                // get_arg_value (func/call.rs) materializes literal arguments
                // directly, even with optimization disabled. Validate their
                // aggregate size here before they can bypass runtime construction.
                if let Some(value) = expression.get_literal_value(None)
                    && let Err(cause) = engine.ensure_data_size_within_limits(&value)
                {
                    failure = Some(compile_error(
                        expression.start_position(),
                        format!("定数のデータが大きさの上限を超えています: {cause}"),
                    ));
                    return false;
                }
            }
            _ => (),
        }
        true
    });
    if let Some(failure) = failure {
        return Err(failure);
    }
    Ok(ast)
}

fn compile_error(position: Position, reason: impl std::fmt::Display) -> String {
    format!(
        "コンパイルに失敗 ({}行{}桁): {reason}",
        position.line().unwrap_or(1),
        position.position().unwrap_or(1)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_script::{
        CompiledOperationScript, ScriptContext, StateSnapshot, build_engine, parse_output,
    };
    use rhai::{Dynamic, Scope};

    #[test]
    fn syntax_rejects_index_property_nested_and_compound_assignments() {
        let engine = build_engine();
        for target in [
            "a[0]",
            "a[0][0]",
            "m[key]",
            "m.k",
            "m.k[0]",
            "a[0].k",
            "m[key][0]",
            "m.k.x",
        ] {
            for operator in [
                "=", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", "&=", "|=", "^=",
            ] {
                let source = format!(
                    "let a = [[0]]; let m = #{{ k: [0] }}; let key = \"k\"; {target} {operator} 1;"
                );
                let error = compile_checked(&engine, &source).unwrap_err();
                assert!(
                    error.contains("添字・プロパティへの代入は使えません"),
                    "{source}: {error}"
                );
            }
        }
    }

    #[test]
    fn syntax_walk_checks_uninvoked_functions_and_nested_statement_blocks() {
        let engine = build_engine();
        for source in [
            "fn slots() { [] } fn unused(s) { s.item.name = \"changed\"; }",
            "fn slots() { [] } fn unused(s) { if false { s[0] += 1; } }",
            "fn slots() { [] } fn unused(s) { for n in 0..1 { s.k[0] = 0; } }",
            "fn slots() { [] } fn unused(s) { let x = { s[0] = 0; 1 }; }",
        ] {
            let error = compile_checked(&engine, source).unwrap_err();
            assert!(
                error.contains("添字・プロパティへの代入は使えません"),
                "{error}"
            );
        }
    }

    #[test]
    fn syntax_rejects_container_mutation_before_slots_evaluation_with_position() {
        let source = "fn slots() { slots_should_not_execute(); }\nfn unused(s) {\n    s.item.name = \"changed\";\n}";
        let error = CompiledOperationScript::compile(source).err().unwrap();
        assert!(error.contains("3行5桁"), "{error}");
        assert!(
            error.contains("添字・プロパティへの代入は使えません"),
            "{error}"
        );
        assert!(!error.contains("slots_should_not_execute"), "{error}");
    }

    #[test]
    fn syntax_rejects_parent_container_growth_and_map_insert_examples() {
        let engine = build_engine();
        for source in [
            "let leaf = []; for n in 0..600 { leaf += 0; } let a = [[], []]; a[0] = leaf; a[1] = leaf;",
            "let m = #{}; for n in 0..1001 { m[n] = (); }",
            "let m = #{}; m[\"new key\"] = ();",
            "fn slots() { let m = #{}; m.extra = (); [] }",
        ] {
            let error = compile_checked(&engine, source).unwrap_err();
            assert!(
                error.contains("添字・プロパティへの代入は使えません"),
                "{error}"
            );
        }
    }

    #[test]
    fn syntax_rejects_this_assignment_through_element_methods() {
        let engine = build_engine();
        for source in [
            "fn grow() { this += [0]; } let a = [[]]; a[0].grow();",
            "fn replace() { this = [0]; } let m = #{ k: [] }; m.k.replace();",
            "fn grow() { this.k += [0]; } let a = [#{ k: [] }]; a[0].grow();",
        ] {
            let error = compile_checked(&engine, source).unwrap_err();
            assert!(
                error.contains("添字・プロパティへの代入は使えません"),
                "{error}"
            );
        }
    }

    #[test]
    fn syntax_allows_variable_assignment_operation_accumulation_and_read_access() {
        let engine = build_engine();
        let source = r#"
            let a = [1, 2]; let m = #{ k: [3] };
            let n = a[0]; n = m.k[0]; n += 1;
            let copy = m.k; copy += 4;
            let ops = []; ops += "FsBgCycle";
            if n == 4 && copy.len() == 2 && m.k.len() == 1 { ops } else { [] }
        "#;
        let ast = compile_checked(&engine, source).unwrap();
        let result = engine.eval_ast::<Dynamic>(&ast).unwrap();
        assert_eq!(parse_output(result).unwrap().actions, ["FsBgCycle"]);

        let script = CompiledOperationScript::compile(
            r#"
            fn slots() { [#{ slot: 1, label: "test", run: "script_entry" }] }
            fn script_entry(s) {
                let ops = []; ops = []; ops += "FsPageNext";
                if s.context == "fs_image" { ops } else { [] }
            }
        "#,
        )
        .unwrap();
        let result = script
            .engine
            .call_fn::<Dynamic>(
                &mut Scope::new(),
                &script.ast,
                "script_entry",
                (StateSnapshot::new(ScriptContext::FsImage).to_dynamic(),),
            )
            .unwrap();
        assert_eq!(parse_output(result).unwrap().actions, ["FsPageNext"]);
    }

    #[test]
    fn syntax_checks_literal_argument_aggregate_sizes_before_function_calls() {
        use crate::operation_script::MAX_STRING_BYTES;
        let engine = build_engine();
        let fields = |count| {
            (0..count)
                .map(|n| format!("k{n}: 0"))
                .collect::<Vec<_>>()
                .join(",")
        };
        for (extra, succeeds) in [(0, true), (1, false)] {
            let arguments = [
                format!(
                    "[[{}],[{}]]",
                    vec!["0"; 500].join(","),
                    vec!["0"; 498 + extra].join(",")
                ),
                format!(
                    "#{{ left: #{{ {} }}, right: #{{ {} }} }}",
                    fields(499),
                    fields(499 + extra)
                ),
                format!(
                    "[\"{}\",\"{}\"]",
                    "a".repeat(MAX_STRING_BYTES / 2),
                    "b".repeat(MAX_STRING_BYTES / 2 + extra)
                ),
                format!(
                    "#{{ left: \"{}\", right: \"{}\" }}",
                    "a".repeat(MAX_STRING_BYTES / 2),
                    "b".repeat(MAX_STRING_BYTES / 2 + extra)
                ),
            ];
            for argument in arguments {
                for source in [
                    format!("fn take_value(x) {{ [] }} take_value({argument});"),
                    format!("fn take_value(x) {{ [] }} fn slots() {{ take_value({argument}) }}"),
                ] {
                    let result = compile_checked(&engine, &source);
                    assert_eq!(result.is_ok(), succeeds, "{result:?}");
                    if let Err(error) = result {
                        assert!(
                            error.contains("定数のデータが大きさの上限を超えています"),
                            "{error}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn syntax_preserves_runtime_checks_for_nonconstant_function_arguments() {
        use crate::operation_script::MAX_STRING_BYTES;
        let engine = build_engine();
        for (extra, succeeds) in [(0, true), (1, false)] {
            let arguments = [
                format!(
                    "[[{}],[{}]]",
                    vec!["zero"; 500].join(","),
                    vec!["zero"; 498 + extra].join(",")
                ),
                // Under OptimizationLevel::None, ${...} remains an Expr::Stmt
                // segment, so interpolation takes the runtime concat checks.
                format!(
                    "`{}${{\"{}\"}}`",
                    "a".repeat(MAX_STRING_BYTES / 2),
                    "b".repeat(MAX_STRING_BYTES / 2 + extra)
                ),
            ];
            for argument in arguments {
                let source =
                    format!("fn take_value(x) {{ x }} let zero = 0; take_value({argument});");
                let ast = compile_checked(&engine, &source).unwrap();
                let result = engine.eval_ast::<Dynamic>(&ast);
                assert_eq!(result.is_ok(), succeeds, "{result:?}");
                if let Err(error) = result {
                    let mut cause = error.as_ref();
                    while let rhai::EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = cause {
                        cause = inner.as_ref();
                    }
                    assert!(
                        matches!(cause, rhai::EvalAltResult::ErrorDataTooLarge(_, _)),
                        "{error}"
                    );
                }
            }
        }
    }
}
