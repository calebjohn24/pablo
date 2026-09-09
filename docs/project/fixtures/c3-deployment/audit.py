"""Audit G01 contract artifacts offline. This is not a deployment resolver.

Uses installed Python 3.11+ tomllib and jsonschema/referencing. Reads only the
schemas and named synthetic corpus files. No environment or credential reads.
"""

import hashlib
import json
from pathlib import Path
import tomllib

from jsonschema import Draft202012Validator
from referencing import Registry, Resource


HERE = Path(__file__).resolve().parent
SCHEMAS = HERE.parent.parent / "schemas"


def canonical(value):
    """The frozen pablo-config-json-v1 byte encoding, independently vectored."""
    if value is None:
        return b"null"
    if type(value) is bool:
        return b"true" if value else b"false"
    if type(value) is int:
        return str(value).encode("ascii")
    if isinstance(value, str):
        parts = ['"']
        for char in value:
            if char in ('"', "\\"):
                parts.append("\\" + char)
            elif ord(char) < 32:
                parts.append(f"\\u{ord(char):04x}")
            else:
                parts.append(char)
        return ("".join(parts) + '"').encode("utf-8")
    if isinstance(value, list):
        return b"[" + b",".join(canonical(item) for item in value) + b"]"
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        keys = sorted(value, key=lambda key: key.encode("utf-8"))
        return b"{" + b",".join(canonical(key) + b":" + canonical(value[key]) for key in keys) + b"}"
    raise TypeError("unsupported configuration data type")


def digest(value):
    return "sha256:" + hashlib.sha256(canonical(value)).hexdigest()


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def terminal_pointers(value, pointer="/config"):
    """Require leaf/empty collection origins and the origin of every list item."""
    if isinstance(value, dict) and value:
        for key, item in value.items():
            escaped = key.replace("~", "~0").replace("/", "~1")
            yield from terminal_pointers(item, pointer + "/" + escaped)
    elif isinstance(value, list) and value:
        for index, item in enumerate(value):
            child = pointer + "/" + str(index)
            yield child
            yield from terminal_pointers(item, child)
    else:
        yield pointer


def main():
    document = load(SCHEMAS / "deployment-v1.schema.json")
    resolved = load(SCHEMAS / "resolved-deployment-v1.schema.json")
    for schema in (document, resolved):
        Draft202012Validator.check_schema(schema)
    registry = Registry().with_resources(
        (schema["$id"], Resource.from_contents(schema)) for schema in (document, resolved)
    )
    validator = Draft202012Validator(document, registry=registry)
    Draft202012Validator(
        {"$ref": document["$id"] + "#/$defs/resolvedOptions"}, registry=registry
    ).validate(load(SCHEMAS / "deployment-defaults-v1.json"))

    cases = load(HERE / "cases.json")
    for case in cases["shape_cases"]:
        raw = (HERE / case["file"]).read_bytes()
        assert len(raw) <= 1048576
        try:
            parsed = tomllib.loads(raw.decode("utf-8").replace("\r\n", "\n"))
        except tomllib.TOMLDecodeError:
            result = "parse_error"
        else:
            try:
                canonical(parsed)  # TOML floats/dates are outside the data subset.
            except TypeError:
                result = "type_error"
            else:
                result = "accepted_shape" if validator.is_valid(parsed) else "schema_error"
        assert result == case["expected"], (case["file"], result)

    for vector in load(HERE / "canonical-vectors.json"):
        actual = canonical(vector["value"])
        assert actual.decode("utf-8") == vector["canonical"]
        assert digest(vector["value"]) == vector["sha256"]

    example = load(HERE / "production.resolved.json")
    Draft202012Validator(resolved, registry=registry).validate(example)
    payload = {key: example[key] for key in ("schema_version", "contract_revision", "config")}
    assert digest(payload) == example["fingerprint"]
    inputs = {key: example[key] for key in ("schema_version", "contract_revision", "sources")}
    assert digest(inputs) == example["input_fingerprint"]
    origins = {source["id"] for source in example["sources"]}
    assert len(origins) == len(example["sources"])
    assert [source["id"] for source in example["sources"]] == [
        f"source-{index:04d}" for index in range(len(example["sources"]))
    ]
    for contributions in example["provenance"].values():
        assert all(contribution["source"] in origins for contribution in contributions)
    assert set(terminal_pointers(example["config"])) <= set(example["provenance"])
    for pointer in example["provenance"]:
        value = example
        for segment in pointer[1:].split("/"):
            key = segment.replace("~1", "/").replace("~0", "~")
            value = value[int(key)] if isinstance(value, list) else value[key]
    for source in example["sources"]:
        if source["kind"] == "file":
            assert source["digest"] == "sha256:" + hashlib.sha256((HERE / source["locator"]).read_bytes()).hexdigest()
        elif source["kind"] == "defaults":
            assert source["digest"] == digest({
                "options": load(SCHEMAS / "deployment-defaults-v1.json"),
                "deployment": {"locked": False, "allowed_run_overrides": ["input"]},
            })
        elif source["kind"] == "profile":
            file, name = source["locator"].split("#")
            declared = tomllib.loads((HERE / file).read_text(encoding="utf-8"))["profiles"][name]
            assert source["digest"] == digest({"name": name, "profile": declared})

    # Check semantic input descriptions, not their future resolver outcomes.
    semantic = cases["resolution_cases"]
    ids = [case["id"] for case in semantic]
    assert len(ids) == len(set(ids))
    assert all(case["owner"] in ("C3.2", "C3.3") and case["status"] == "not_run" for case in semantic)
    assert all(case["input"] and case["expected"] for case in semantic)
    print(f"G01: 2 schemas, baseline defaults, {len(cases['shape_cases'])} TOML shape cases, "
          f"3 canonical vectors and one resolved/provenance example passed artifact audit.")
    print(f"G02/G03: {len(semantic)} specified semantic cases remain NOT RUN; no loader, "
          "authority enforcement, credentials, live providers or runtime interfaces were executed.")


if __name__ == "__main__":
    main()
