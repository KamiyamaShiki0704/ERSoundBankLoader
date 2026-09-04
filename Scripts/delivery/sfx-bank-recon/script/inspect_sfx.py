"""Read-only PE reconnaissance. Results are candidates, not callable ABI declarations."""

import argparse
import bisect
import hashlib
import json
import re
import struct
from pathlib import Path

import capstone
import pefile
from capstone.x86 import X86_OP_IMM, X86_OP_MEM, X86_REG_RIP


class Binary:
    def __init__(self, path):
        self.path = Path(path)
        self.data = self.path.read_bytes()
        self.pe = pefile.PE(data=self.data)
        self.base = self.pe.OPTIONAL_HEADER.ImageBase
        self.text = next(s for s in self.pe.sections if s.Name.rstrip(b"\0") == b".text")
        self.code = self.text.get_data()
        # pefile caps parsed exception entries; ER has more than 230,000 of them.
        directory = self.pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]
        pdata = self.pe.get_data(directory.VirtualAddress, directory.Size)
        self.functions = sorted((start, end) for start, end, _ in
                                struct.iter_unpack("<III", pdata) if start < end)
        self.starts = [f[0] for f in self.functions]
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True
        self.cache = {}

    def bounds(self, rva):
        index = bisect.bisect_right(self.starts, rva) - 1
        if index >= 0 and rva < self.functions[index][1]:
            return self.functions[index]
        return None

    def instructions(self, bounds):
        if bounds not in self.cache:
            start, end = bounds
            self.cache[bounds] = list(self.md.disasm(self.pe.get_data(start, end - start), start))
        return self.cache[bounds]

    def describe(self, ins):
        result = {"rva": hex(ins.address), "instruction": f"{ins.mnemonic} {ins.op_str}"}
        bounds = self.bounds(ins.address)
        if bounds:
            result["function"] = [hex(v) for v in bounds]
        return result

    def strings(self, pattern):
        result = []
        search = re.compile(pattern, re.I)
        for section in self.pe.sections:
            data = section.get_data()
            for regex, encoding in [(rb"[\x20-\x7e]{5,}", "ascii"),
                                    (rb"(?:[\x20-\x7e]\x00){5,}", "utf-16-le")]:
                for match in re.finditer(regex, data):
                    value = match.group().decode(encoding)
                    if search.search(value):
                        result.append({"rva": hex(section.VirtualAddress + match.start()),
                                       "encoding": encoding, "value": value})
        return result

    def candidates(self, regex):
        bounds = set()
        for match in re.finditer(regex, self.code, re.DOTALL):
            bound = self.bounds(self.text.VirtualAddress + match.start())
            if bound:
                bounds.add(bound)
        return bounds

    def references(self, targets, calls=False):
        pattern = rb"\xe8...." if calls else rb"[\x40-\x4f]?[\x8b\x8d\x89][\x05\x0d\x15\x1d\x25\x2d\x35\x3d]...."
        bounds = set()
        for match in re.finditer(pattern, self.code, re.DOTALL):
            target = self.text.VirtualAddress + match.end() + struct.unpack("<i", match.group()[-4:])[0]
            if target in targets:
                bound = self.bounds(self.text.VirtualAddress + match.start())
                if bound:
                    bounds.add(bound)
        result = []
        for bound in sorted(bounds):
            for ins in self.instructions(bound):
                for operand in ins.operands:
                    target = None
                    if calls and ins.mnemonic == "call" and operand.type == X86_OP_IMM:
                        target = operand.imm
                    elif not calls and operand.type == X86_OP_MEM and operand.mem.base == X86_REG_RIP:
                        target = ins.address + ins.size + operand.mem.disp
                    if target in targets:
                        result.append({**self.describe(ins), "target": hex(target)})
        return result

    def field_reads(self, offset):
        result = []
        for bound in sorted(self.candidates(re.escape(struct.pack("<I", offset)))):
            for ins in self.instructions(bound):
                if ins.mnemonic not in {"movsx", "movzx", "mov", "cmp"}:
                    continue
                if any(op.type == X86_OP_MEM and op.mem.disp == offset and op.size == 2
                       for op in ins.operands):
                    result.append(self.describe(ins))
        return result


def layout(path, field):
    body = Path(path).read_text(encoding="utf-8").split("pub struct NPC_PARAM_ST {", 1)[1].split("\n}", 1)[0]
    offset = 0
    max_alignment = 1
    result = None
    sizes = {"u8": 1, "i8": 1, "u16": 2, "i16": 2,
             "u32": 4, "i32": 4, "f32": 4, "u64": 8, "i64": 8, "f64": 8}
    for name, kind in re.findall(r"^\s+(\w+): ([^,]+),", body, re.M):
        array = re.fullmatch(r"\[(\w+); (\d+)\]", kind)
        base, count = (array[1], int(array[2])) if array else (kind, 1)
        alignment = sizes[base]
        offset = (offset + alignment - 1) // alignment * alignment
        if name == field:
            result = {"field": field, "offset": hex(offset), "type": kind}
        offset += alignment * count
        max_alignment = max(max_alignment, alignment)
    if result is None:
        raise ValueError(f"Missing field: {field}")
    return {**result, "total_size": (offset + max_alignment - 1) // max_alignment * max_alignment,
            "source": str(path), "method": "repr(C) primitive field alignment"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["strings", "xrefs", "callers", "field", "disasm", "layout"])
    parser.add_argument("--exe", type=Path)
    parser.add_argument("--pattern", default="ffxbnd|sfxbnd|SfxResBank|FfxRepository")
    parser.add_argument("--rva", type=lambda x: int(x, 0), action="append", default=[])
    parser.add_argument("--offset", type=lambda x: int(x, 0))
    parser.add_argument("--size", type=lambda x: int(x, 0), help="Explicit disassembly byte count for leaf functions")
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--field", default="sfx_res_bank_id")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.mode == "layout":
        report = layout(args.rust, args.field)
    else:
        binary = Binary(args.exe)
        if args.mode == "strings":
            findings = binary.strings(args.pattern)
        elif args.mode in {"xrefs", "callers"}:
            findings = binary.references(set(args.rva), args.mode == "callers")
        elif args.mode == "field":
            findings = binary.field_reads(args.offset)
        else:
            findings = []
            for rva in args.rva:
                bound = (rva, rva + args.size) if args.size else binary.bounds(rva)
                if bound is None:
                    raise ValueError(f"No unwind function contains {rva:#x}")
                findings.extend(binary.describe(ins) for ins in binary.instructions(bound))
        report = {"exe": str(args.exe.resolve()), "sha256": hashlib.sha256(binary.data).hexdigest(),
                  "image_base": hex(binary.base), "mode": args.mode, "findings": findings}
    output = json.dumps(report, indent=2, ensure_ascii=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + "\n", encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
