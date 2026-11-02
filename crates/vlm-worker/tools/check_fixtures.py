"""Check public prompt/schema constants and compare the server's schema subgrammar."""
import argparse
import json
import pathlib
import runpy


def literal(text, delimiter):
    return text.split('R"' + delimiter + '(', 1)[1].split(')' + delimiter + '"', 1)[0]


def rules(text):
    return dict(line.split(" ::= ", 1) for line in text.splitlines() if " ::= " in line)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server-grammar", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    worker = pathlib.Path(__file__).resolve().parents[1]
    repo = worker.parents[1]
    evaluation = runpy.run_path(str(repo / "scripts/vlm-eval/run_eval.py"))
    text = (worker / "src/prompt.h").read_text(encoding="utf-8")
    prompt_equal = literal(text, "PROMPT") == evaluation["PROMPTS"]["v0-3"]
    schema_equal = json.loads(literal(text, "SCHEMA")) == evaluation["SCHEMAS"]["v0-3l"]
    direct = literal((worker / "src/caption-grammar.h").read_text(encoding="utf-8"), "GBNF")
    server = pathlib.Path(a.server_grammar).read_text(encoding="utf-8")
    direct_rules = rules(direct)
    subset = {}
    for name, value in rules(server).items():
        if name.startswith("response-format") or name in ("char", "space"):
            name = name.replace("response-format-", "").replace("response-format", "root")
            value = value.replace("response-format-", "").replace("response-format", "root")
            subset[name] = value
    result = {"prompt_v0_3_equal": prompt_equal, "schema_v0_3l_equal": schema_equal,
              "whole_grammar_equal": direct == server, "schema_subgrammar_equal": direct_rules == subset,
              "difference": "Server grammar includes assistant/thinking prefix prefilled by common. Its post-prefill whitespace state can differ from the worker's direct JSON root; schema-subgrammar equality does not prove full state equivalence."}
    pathlib.Path(a.out).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    if not (prompt_equal and schema_equal and result["schema_subgrammar_equal"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
