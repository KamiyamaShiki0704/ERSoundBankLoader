"""Compare ER 2.7.1 native SFX instructions with retained 2.7.0 JSON evidence."""
import argparse
import hashlib
import json
import re
from pathlib import Path

from inspect_sfx import Binary


CASES = [
    ("acquire", "request-release.json", 0xD90CF0, 0xD90D60, 0x208),
    ("release", "request-release.json", 0xD91020, 0xD91090, 0x56),
    ("factory", "filecap-factory.json", 0x1F1D10, 0x1F1D10, 0x123),
    ("collector", "sfx-gc.json", 0xD931C0, 0xD93230, 0x298),
    ("processing", "ffxbnd-processing.json", 0x20F340, 0x20F340, 0xD2B),
]


def compare(binary, evidence, filename, old_start, new_start, size):
    source = json.loads((evidence / filename).read_text())
    assert source["sha256"].lower() == "d1a84083c6c7c7902162ff098f7d86812839aa6b3575959398857e539c488134"
    old = [i for i in source["findings"] if old_start <= int(i["rva"], 16) < old_start + size]
    new = binary.instructions((new_start, new_start + size))
    assert len(old) == len(new), "Instruction count changed"
    changes = []
    for prior, current in zip(old, new):
        before, after = prior["instruction"], f"{current.mnemonic} {current.op_str}"
        assert int(prior["rva"], 16) - old_start == current.address - new_start
        if before == after:
            continue
        kind = "unexpected"
        if before.startswith("call 0x") and after.startswith("call 0x"):
            if int(after.split()[1], 16) - int(before.split()[1], 16) == 0x70:
                kind = "relocated_call"
        elif current.mnemonic.startswith("j"):
            parts = before.split()
            if parts[0] == current.mnemonic and int(parts[1], 16) - old_start == int(current.op_str, 16) - new_start:
                kind = "same_local_branch"
        elif "[rip + " in before and "[rip + " in after:
            pattern = r"\[rip \+ (0x[0-9a-f]+)\]"
            a, b = re.search(pattern, before), re.search(pattern, after)
            if a and b and re.sub(pattern, "[rip]", before) == re.sub(pattern, "[rip]", after):
                if int(a[1], 16) + old_start == int(b[1], 16) + new_start:
                    kind = "same_rip_target"
        assert kind != "unexpected", (prior, after)
        changes.append({"old": prior["rva"], "new": hex(current.address), "kind": kind,
                        "before": before, "after": after})
    return {"old_rva": hex(old_start), "new_rva": hex(new_start),
            "instructions": len(new), "changes": changes,
            "non_address_instructions_unchanged": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = Binary(args.exe)
    digest = hashlib.sha256(binary.data).hexdigest()
    assert digest == "1a3547101327f65d0c76da2f9190ac0aa66871ea42bae2aecc61e11a8b597891"
    findings = {name: compare(binary, args.evidence, file, old, new, size)
                for name, file, old, new, size in CASES}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"sha256": digest, "findings": findings}, indent=2) + "\n")
    for name, finding in findings.items():
        print(name, finding["old_rva"], "->", finding["new_rva"],
              "instructions:", finding["instructions"], "PASS")


if __name__ == "__main__":
    main()
