#pragma once
// Generated offline by vlm-reference using b12032 json_schema_to_grammar.
inline constexpr char kGrammar[] = R"GBNF(caption ::= "\"" char{0,400} "\""
caption-kv ::= "\"caption\"" space ":" space caption
char ::= [^"\\\x7F\x00-\x1F] | [\\] (["\\bfnrt] | "u" [0-9a-fA-F]{4})
keywords ::= "[" space (keywords-item ("," space keywords-item){0,19})? space "]"
keywords-item ::= "\"" char{0,32} "\""
keywords-kv ::= "\"keywords\"" space ":" space keywords
root ::= "{" space caption-kv "," space text-kv "," space keywords-kv space "}"
space ::= | " " | "\n"{1,2} [ \t]{0,20}
text ::= "\"" char{0,400} "\""
text-kv ::= "\"text\"" space ":" space text
)GBNF";
