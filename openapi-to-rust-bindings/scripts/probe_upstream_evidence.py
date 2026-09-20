#!/usr/bin/env python3
"""Bounded #148 research experiment; NOT a production parser or Bindings emitter.

A documented literal route is correlated with an exact effective OpenAPI
operation only if the generated method body independently contains that literal
URL and an HTTP method call. Ambiguity fails closed; #149/#150 replace this
lexical probe with structured Rust AST and transport evidence.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys


class ProbeError(ValueError):
    pass


METHOD_DOC = re.compile(
    r"(?m)^[ \t]*///[ \t]*\x60"
    r"(?P<verb>GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|TRACE)"
    r"[ \t]+(?P<route>[^\x60\r\n]+)\x60[ \t]*\r?\n"
    r"[ \t]*pub async fn[ \t]+(?P<rust_name>[A-Za-z_][A-Za-z_0-9]*)[ \t]*\("
)
HTTP_METHODS = {"get", "post", "put", "patch", "delete", "head", "options", "trace"}


def source_operations(openapi: dict) -> dict[tuple[str, str], dict]:
    paths = openapi.get("paths")
    if not isinstance(paths, dict):
        raise ProbeError("source OpenAPI has no paths object")
    sources = {}
    for path, item in paths.items():
        if not isinstance(item, dict):
            raise ProbeError(f"unsupported path item: {path!r}")
        for verb, definition in item.items():
            if verb.lower() not in HTTP_METHODS:
                continue
            if not isinstance(definition, dict) or not isinstance(
                definition.get("operationId"), str
            ) or not definition["operationId"].strip():
                raise ProbeError(f"operationId missing: {verb.upper()} {path}")
            sources[(verb.upper(), path)] = definition
    if not sources:
        raise ProbeError("no supported OpenAPI operations")
    return sources


def correlate(openapi: dict, client: str) -> dict:
    sources = source_operations(openapi)
    docs = list(METHOD_DOC.finditer(client))
    if not docs:
        raise ProbeError("no adjacent operation doc and public Rust method")
    evidence = []
    seen = set()
    for i, match in enumerate(docs):
        verb, path, name = (
            match.group("verb"), match.group("route"), match.group("rust_name")
        )
        key = (verb, path)
        if key not in sources:
            raise ProbeError(
                f"source_identity_ambiguous: {name}: {verb} {path} "
                "has no exact effective OpenAPI operation"
            )
        if key in seen:
            raise ProbeError(
                f"source_identity_ambiguous: two generated methods claim {verb} {path}; "
                "multiple representations require #150"
            )
        seen.add(key)
        stop = docs[i + 1].start() if i + 1 < len(docs) else len(client)
        section = client[match.end():stop]
        if json.dumps(path) not in section:
            raise ProbeError(
                f"source_identity_ambiguous: {name}: no matching literal URL "
                "in the generated method body"
            )
        if not re.search(rf"\.\s*{verb.lower()}\s*\(\s*request_url\s*\)", section):
            raise ProbeError(
                f"source_identity_ambiguous: {name}: no matching HTTP call "
                "on request_url in the generated body"
            )
        signature = re.search(
            r"\)\s*->\s*(?P<return_type>[^{]+)\{", section, re.DOTALL
        )
        if signature is None:
            raise ProbeError(f"signature_unsupported: {name}")
        response_media = sorted({
            media
            for response in sources[key].get("responses", {}).values()
            if isinstance(response, dict)
            for media in response.get("content", {})
        })
        evidence.append({
            "source_operation_id": sources[key]["operationId"],
            "source_http_method": verb,
            "source_path": path,
            "observed_method_name": name,
            "observed_return_type": " ".join(signature["return_type"].split()),
            "declared_response_media": response_media,
            "bytes_stream_call_observed": ".bytes_stream()" in section,
            "sse_accept_observed": bool(
                re.search(r'ACCEPT\s*,\s*"text/event-stream"', section)
            ),
            "emitted_operation_id": "unproven (do not derive from method name)",
            "representation": "unproven pending #150",
            "success_statuses": "unproven pending #150",
        })
    missing = sorted(set(sources) - seen)
    if missing:
        raise ProbeError(
            "source_operation_unemitted: " + ", ".join(
                f"{verb} {path}" for verb, path in missing
            )
        )
    return {
        "scope": "literal URL + HTTP verb corroboration only; no Bindings emitted",
        "method_count": len(evidence),
        "operations": evidence,
    }


def self_test() -> None:
    spec = {"paths": {"/one": {"get": {
        "operationId": "OriginalIDNotMethodName",
        "responses": {"200": {"description": "ok"}}
    }}}}
    tick = chr(96)
    method = (
        f"    /// {tick}GET /one{tick}\n"
        "    pub async fn renamed(&self) -> Result<(), Error> {\n"
        '        let request_url = format!("{}{}", self.base_url, "/one");\n'
        "        let req = self.http_client.get(request_url);\n"
        "    }\n"
    )
    result = correlate(spec, method)
    assert result["operations"][0]["source_operation_id"] == "OriginalIDNotMethodName"
    failures = (
        method + method.replace("renamed", "renamed_two"),
        method.replace('"/one"', '"/elsewhere"'),
        method.replace(".get(request_url)", ".post(request_url)"),
        method.replace("GET /one", "GET /unknown"),
    )
    for bad in failures:
        try:
            correlate(spec, bad)
        except ProbeError:
            continue
        raise AssertionError("ambiguous or uncorroborated source was accepted")
    print("probe self-test: renamed ID and four fail-closed cases passed")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--openapi", type=Path)
    parser.add_argument("--generated", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.openapi is None or args.generated is None:
        parser.error("--openapi and --generated required unless --self-test")
    if (args.generated / "binding-manifest.json").exists():
        raise ProbeError("producer manifest is forbidden in upstream-only probe")
    if not (args.generated / "types.rs").is_file():
        raise ProbeError("ordinary upstream types.rs is missing")
    result = correlate(
        json.loads(args.openapi.read_text()),
        (args.generated / "client.rs").read_text(),
    )
    report = json.dumps(result, indent=2) + "\n"
    if args.report:
        args.report.write_text(report)
    print(report, end="")


if __name__ == "__main__":
    try:
        main()
    except (ProbeError, OSError, json.JSONDecodeError) as exc:
        print(f"upstream evidence probe: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
