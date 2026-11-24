//! Reject container mutations before any script code (including slots()) runs.

use rhai::{AST, ASTNode, Engine, Expr, Position, Stmt, Token};

const MAX_MAP_KEY_BYTES: usize = 256;
const ASSIGNMENT_ERROR: &str = "添字・プロパティへの代入は使えません。thisへの代入も含め、変数そのものへの代入だけが使用できます";
const BREAK_VALUE_ERROR: &str =
    "値付きのbreakは使えません。breakの直後は ; または } にしてください";

/// The worker's only compilation path. Optimization must remain disabled so
/// literal aggregate checks can inspect the original expressions. Assignment
/// checks primarily use tokens: Rhai 1.27.0's Stmt::walk skips break values.
pub(super) fn compile_checked(engine: &Engine, source: &str) -> Result<AST, String> {
    check_tokens(engine, source)?;
    let ast = engine
        .compile(source)
        .map_err(|error| format!("コンパイルに失敗: {error}"))?;
    let mut failure = None;
    ast.walk(&mut |path| {
        failure = path.last().and_then(|node| check_node(engine, node).err());
        failure.is_none()
    });
    if let Some(failure) = failure {
        return Err(failure);
    }
    Ok(ast)
}

fn check_node(engine: &Engine, node: &ASTNode<'_>) -> Result<(), String> {
    match node {
        ASTNode::Stmt(Stmt::Assignment(assignment)) => {
            // The parser uses the same Assignment node for = and every op-assignment.
            // A chain's root is Index/Dot even for nested/mixed access paths.
            // ThisPtr is also forbidden: a[0].method() can bind `this` directly to
            // an element, so assigning `this` would mutate the parent indirectly.
            if !matches!(&assignment.1.lhs, Expr::Variable(..)) {
                return Err(compile_error(
                    assignment.1.lhs.start_position(),
                    ASSIGNMENT_ERROR,
                ));
            }
        }
        ASTNode::Stmt(Stmt::BreakLoop(Some(_), _, position)) => {
            return Err(compile_error(*position, BREAK_VALUE_ERROR));
        }
        ASTNode::Stmt(Stmt::Switch(switch, _)) => {
            // Stmt::walk follows only case/range/default indices. Empty
            // ranges leave unreferenced guard/action expressions in the AST;
            // walk those too so even unreachable literals obey our limits.
            let cases = &switch.1;
            let mut referenced = vec![false; cases.expressions.len()];
            for case in cases.cases.values() {
                for &index in &case.blocks {
                    referenced[index] = true;
                }
            }
            for range in &cases.ranges {
                referenced[range.index()] = true;
            }
            if let Some(index) = cases.def_case {
                referenced[index] = true;
            }
            for (index, expression) in cases.expressions.iter().enumerate() {
                if !referenced[index] {
                    check_expression_tree(engine, &expression.lhs)?;
                    check_expression_tree(engine, &expression.rhs)?;
                }
            }
        }
        ASTNode::Expr(expression) => {
            // Map keys are absent from Rhai's aggregate string-size check.
            // Check the parsed UTF-8 key and retain its own source position.
            // AST::walk also visits nested maps and every function body.
            if let Expr::Map(map, _) = expression {
                for (key, _) in &map.0 {
                    if key.as_str().len() > MAX_MAP_KEY_BYTES {
                        return Err(compile_error(
                            key.pos,
                            format!("マップのキーは{MAX_MAP_KEY_BYTES}バイト以内にしてください"),
                        ));
                    }
                }
            }
            // get_arg_value (func/call.rs) materializes literal arguments
            // directly, even with optimization disabled. Validate their
            // aggregate size here before they can bypass runtime construction.
            if matches!(
                expression,
                Expr::Array(..) | Expr::Map(..) | Expr::InterpolatedString(..)
            ) && let Some(value) = expression.get_literal_value(None)
                && let Err(cause) = engine.ensure_data_size_within_limits(&value)
            {
                return Err(compile_error(
                    expression.start_position(),
                    format!("定数のデータが大きさの上限を超えています: {cause}"),
                ));
            }
        }
        _ => (),
    }
    Ok(())
}

fn check_expression_tree(engine: &Engine, expression: &Expr) -> Result<(), String> {
    let mut failure = None;
    expression.walk(&mut Vec::new(), &mut |path| {
        failure = path.last().and_then(|node| check_node(engine, node).err());
        failure.is_none()
    });
    failure.map_or(Ok(()), Err)
}

fn check_tokens(engine: &Engine, source: &str) -> Result<(), String> {
    let (mut lexer, control) = engine.lex([&source]);
    let mut tokens = Vec::new();
    let mut brace_depth = 0_usize;
    let mut interpolations = Vec::new();
    // Closing square brackets retain their matching opener for diagnostics.
    let mut brackets = Vec::new();
    let mut openers = Vec::new();
    while let Some((token, position)) = lexer.next() {
        match &token {
            // Rhai's lexer yields EOF repeatedly; do not collect it indefinitely.
            Token::EOF => break,
            Token::LexError(error) => return Err(compile_error(position, error)),
            Token::Comment(_) => continue,
            Token::InterpolatedString(_) => interpolations.push(brace_depth),
            Token::LeftBrace | Token::MapStart => brace_depth += 1,
            Token::RightBrace => {
                brace_depth = brace_depth.saturating_sub(1);
                if interpolations.last() == Some(&brace_depth) {
                    interpolations.pop();
                    // As in parser.rs::parse_primary, resume text only after the
                    // interpolation block. Nested maps/blocks/strings have their
                    // own depths; text containing operators is never code.
                    control.borrow_mut().is_within_text = true;
                }
            }
            _ => (),
        }
        let opener = match &token {
            Token::LeftBracket | Token::QuestionBracket => {
                brackets.push(tokens.len());
                None
            }
            Token::RightBracket => brackets.pop(),
            _ => None,
        };
        tokens.push((token, position));
        openers.push(opener);
    }

    for (index, (token, position)) in tokens.iter().enumerate() {
        if matches!(token, Token::Break)
            && !matches!(
                tokens.get(index + 1).map(|(next, _)| next),
                Some(Token::SemiColon | Token::RightBrace)
            )
        {
            return Err(compile_error(*position, BREAK_VALUE_ERROR));
        }
        if !matches!(token, Token::Equals) && !token.is_op_assignment() {
            continue;
        }
        let Some(mut tail) = index.checked_sub(1) else {
            continue;
        };
        // Rhai strips grouping parentheses from the LHS, e.g. ((a[0])) = x.
        while tail > 0 && matches!(tokens[tail].0, Token::RightParen) {
            tail -= 1;
        }
        let property = |i: usize| {
            i > 0
                && matches!(tokens[i].0, Token::Identifier(_) | Token::Reserved(_))
                && matches!(tokens[i - 1].0, Token::Period | Token::Elvis)
        };
        let is_this = matches!(&tokens[tail].0, Token::Reserved(name) if name.as_str() == "this");
        if matches!(tokens[tail].0, Token::RightBracket) || property(tail) || is_this {
            // Walk backwards through a chain without recursive calls, including
            // parenthesized bases, to keep the diagnostic at its root variable.
            loop {
                match &tokens[tail].0 {
                    Token::RightParen if tail > 0 => tail -= 1,
                    Token::RightBracket => match openers[tail].and_then(|i| i.checked_sub(1)) {
                        Some(base) => tail = base,
                        None => break,
                    },
                    _ if property(tail) && tail >= 2 => tail -= 2,
                    _ => break,
                }
            }
            return Err(compile_error(tokens[tail].1, ASSIGNMENT_ERROR));
        }
    }
    Ok(())
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
    fn syntax_map_literal_keys_enforce_utf8_byte_limit_in_all_bodies() {
        assert_eq!(MAX_MAP_KEY_BYTES, 256);
        for (key, succeeds) in [
            ("k".repeat(256), true),
            ("k".repeat(257), false),
            (format!("{}a", "界".repeat(85)), true),
            (format!("{}aa", "界".repeat(85)), false),
        ] {
            let mut spellings = vec![format!("\"{key}\"")];
            if key.is_ascii() {
                spellings.push(key);
            }
            for spelling in spellings {
                for source in [
                    format!("let m = #{{ {spelling}: 0 }}; fn slots() {{ [] }}"),
                    format!("const m = #{{ outer: #{{ {spelling}: 0 }} }}; fn slots() {{ [] }}"),
                    format!("fn slots() {{ let m = #{{ {spelling}: 0 }}; [] }}"),
                    format!(
                        "fn slots() {{ [] }} fn unused(s) {{ let m = #{{ outer: #{{ {spelling}: s }} }}; }}"
                    ),
                ] {
                    let result = CompiledOperationScript::compile(&source);
                    assert_eq!(result.is_ok(), succeeds, "{source}");
                    if let Err(error) = result {
                        assert!(error.contains("マップのキーは256バイト以内"), "{error}");
                    }
                }
            }
        }
    }

    #[test]
    fn syntax_map_key_failure_reports_key_line_and_column() {
        let source = format!(
            "fn slots() {{ [] }}\nfn unused(s) {{\n    let m = #{{\n        {}: s\n    }};\n}}",
            "k".repeat(257),
        );
        let error = CompiledOperationScript::compile(&source).err().unwrap();
        assert!(error.contains("4行9桁"), "{error}");
        assert!(error.contains("マップのキーは256バイト以内"), "{error}");
    }

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
            "m?.k",
            "(a[0])",
            "((m.k))",
            "(m.k)[0]",
            "this",
            "((this))",
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
    fn syntax_tokens_reject_mutations_in_every_expression_context() {
        let engine = build_engine();
        for body in [
            "a[0] = 1;",
            "switch 0 { 0 => { a[0] += 1; }, _ => () }",
            "let x = { a[0] = 1; 0 };",
            "let x = if true { a[0] = 1; 0 } else { 1 };",
            "let text = `value ${a[0] = 1; 0}`;",
            "let text = `outer ${`inner ${a[0] = 1; 0}`}`;",
            "let text = `outer ${#{ k: { a[0] = 1; 0 } }}`;",
            "a /* ] = this break 1 */ [0] /* comment */ += 1;",
            "m /* comment */ ?. /* comment */ k /* comment */ = 1;",
        ] {
            for source in [
                format!("let a = [0]; let m = #{{ k: 0 }}; {body}"),
                format!("fn slots() {{ [] }} fn unused(a, m) {{ {body} }}"),
            ] {
                let error = compile_checked(&engine, &source).unwrap_err();
                assert!(error.contains(ASSIGNMENT_ERROR), "{source}: {error}");
            }
        }
    }

    #[test]
    fn syntax_tokens_reject_break_values_before_slots_evaluation() {
        for body in [
            "loop { break { a[0] = 1; }; }",
            "loop { break a[0] = 1; }",
            "loop { break 1; }",
            "loop { break (); }",
            "loop { switch 0 { 0 => { break { a[0] = 1; }; }, _ => () } }",
            "loop { let x = { break { a[0] = 1; }; 0 }; }",
            "loop { let x = if true { break { a[0] = 1; }; } else { 0 }; }",
            "loop { let text = `value ${break { a[0] = 1; };}`; }",
        ] {
            let source = format!(
                "fn slots() {{ slots_should_not_execute(); }}\nfn unused(a) {{\n    {body}\n}}"
            );
            let error = CompiledOperationScript::compile(&source).err().unwrap();
            assert!(error.contains(BREAK_VALUE_ERROR), "{source}: {error}");
            assert!(error.contains("3行"), "{error}");
            assert!(!error.contains("slots_should_not_execute"), "{error}");
        }
    }

    #[test]
    fn syntax_tokens_allow_comments_strings_and_interpolation_text() {
        let engine = build_engine();
        for source in [
            r#"// a[0] = 1; break 1;
                /* m.k += 1; /* nested */ this = 1; */
                let ops = []; ops /* a[0] = 1 */ += "FsBgCycle";"#,
            r#"let text = "a[0] = 1; m.k += 1; this = 0; break 1;";"#,
            r#"let text = `a[0] = 1; break 1;`;"#,
            r#"let text = `a[0] = 1; ${1} m.k += 1; ${2} break 1;`;"#,
            r#"let text = `outer ${`inner ${1} a[0] = 1;`} this = 0;`;"#,
            r#"let text = `outer ${#{ k: "}" }.k} break 1;`;"#,
            r#"let text = `outer ${/* } break 1; */ "}"} a[0] = 1;`;"#,
        ] {
            compile_checked(&engine, source).unwrap_or_else(|error| panic!("{source}: {error}"));
        }
        // Resuming text must not swallow the code after the closing backtick.
        for text in ["`text ${1} tail`", "`outer ${`inner ${1}`} tail`"] {
            let source = format!("let a = [0]; let text = {text}; a[0] = 1;");
            assert!(
                compile_checked(&engine, &source)
                    .unwrap_err()
                    .contains(ASSIGNMENT_ERROR)
            );
        }
    }

    #[test]
    fn syntax_tokens_allow_variable_assignments_comparisons_and_loop_control() {
        let engine = build_engine();
        for operator in [
            "=", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", "&=", "|=", "^=",
        ] {
            let source = format!("let n = 1; ((n)) {operator} 1;");
            compile_checked(&engine, &source).unwrap_or_else(|error| panic!("{source}: {error}"));
        }
        let source = r#"
            let a = [1]; let m = #{ k: 1 }; let n = 0;
            loop {
                n += 1;
                if n < 2 { continue; }
                break /* comment */;
            }
            loop { break }
            let ops = []; ops += "FsBgCycle";
            a[0] == 1 && a[0] != 2 && m.k <= 1 && m.k >= 1
                && n == 2 && ops.len() == 1
        "#;
        let ast = compile_checked(&engine, source).unwrap();
        assert!(engine.eval_ast::<bool>(&ast).unwrap());
    }

    #[test]
    fn syntax_walk_checks_literal_limits_in_switch_and_expression_children() {
        let engine = build_engine();
        let oversized = [
            (
                format!(
                    "[[{}],[{}]]",
                    vec!["0"; 500].join(","),
                    vec!["0"; 499].join(",")
                ),
                "定数のデータが大きさの上限を超えています",
            ),
            (
                format!("#{{ \"{}\": 0 }}", "k".repeat(257)),
                "マップのキーは256バイト以内",
            ),
        ];
        for (literal, reason) in oversized {
            for expression in [
                format!("switch 0 {{ 0 => take_value({literal}), _ => 0 }}"),
                format!("switch 0 {{ 0 if take_value({literal}) => 1, _ => 0 }}"),
                format!("switch 0 {{ 1..=2 => take_value({literal}), _ => 0 }}"),
                format!("switch 0 {{ 1..0 => take_value({literal}), _ => 0 }}"),
                format!("switch 0 {{ 1..0 if take_value({literal}) => 1, _ => 0 }}"),
                format!("switch 0 {{ 0 => 1, _ => take_value({literal}) }}"),
                format!("if true {{ take_value({literal}) }} else {{ 0 }}"),
                format!("{{ take_value({literal}); 0 }}"),
                format!("`text ${{take_value({literal})}}`"),
            ] {
                let source = format!("fn unused() {{ let value = {expression}; }}");
                let error = compile_checked(&engine, &source).unwrap_err();
                assert!(error.contains(reason), "{source}: {error}");
            }
        }
    }

    #[test]
    fn syntax_checked_default_template_compiles_and_evaluates_slots() {
        CompiledOperationScript::compile(crate::operation_script::DEFAULT_SCRIPT).unwrap();
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
