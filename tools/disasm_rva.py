"""把 PE 里某个 RVA 附近的机器码反汇编出来（排查崩溃用，需要 capstone）。

用法:
  python tools/disasm_rva.py <exe> 0x935D [0x40A3 ...]          # 看 RVA 附近
  python tools/disasm_rva.py <exe> --func 0x935D                # 整个函数（用 .pdata 定边界）
  python tools/disasm_rva.py <exe> --func 0x935D --grep "0x1c8" # 只看匹配的行
"""
import struct
import sys

from capstone import Cs, CS_ARCH_X86, CS_MODE_64


def sections(path):
    data = open(path, "rb").read()
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[e_lfanew : e_lfanew + 4] == b"PE\0\0", "不是 PE 文件"
    coff = e_lfanew + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    base = struct.unpack_from("<Q", data, coff + 20 + 24)[0]  # ImageBase (PE32+)
    out = []
    off = coff + 20 + opt_size
    for i in range(nsec):
        h = off + i * 40
        name = data[h : h + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, va, rawsize, raw = struct.unpack_from("<IIII", data, h + 8)
        out.append((name, va, vsize, raw, rawsize))
    return data, base, out


def rva_to_off(secs, rva):
    for name, va, vsize, raw, rawsize in secs:
        if va <= rva < va + max(vsize, rawsize):
            return raw + (rva - va)
    return None


def functions(secs, data):
    """从 .pdata 解析函数边界（RUNTIME_FUNCTION 是 12 字节一条）。"""
    for name, va, vsize, raw, rawsize in secs:
        if name != ".pdata":
            continue
        out = []
        for off in range(raw, raw + rawsize, 12):
            begin, end, _unwind = struct.unpack_from("<III", data, off)
            if begin:
                out.append((begin, end))
        out.sort()
        return out
    return []


def main():
    path = sys.argv[1]
    args = sys.argv[2:]
    whole_func = "--func" in args
    grep = None
    if "--grep" in args:
        i = args.index("--grep")
        grep = args[i + 1].replace("0x", "").lower()
        args = args[:i] + args[i + 2 :]
    rvas = [int(a, 16) for a in args if not a.startswith("--")]
    data, base, secs = sections(path)
    print(f"# {path} ImageBase=0x{base:X} 段: {[(n, hex(v)) for n, v, *_ in secs]}")
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    funcs = functions(secs, data)
    for rva in rvas:
        if whole_func:
            hit = [f for f in funcs if f[0] <= rva < f[1]]
            if not hit:
                print(f"--- RVA 0x{rva:X}: .pdata 里找不到所属函数")
                continue
            start, end = hit[0]
            print(f"--- 函数 RVA 0x{start:X}..0x{end:X}（长度 0x{end - start:X}）")
        else:
            start = max(0, rva - 0x60)
            end = rva + 0x60
        off = rva_to_off(secs, start)
        if off is None:
            print(f"--- RVA 0x{rva:X}: 不在任何段内")
            continue
        code = data[off : off + (end - start)]
        if not whole_func:
            print(f"--- RVA 0x{rva:X} 附近")
        for ins in md.disasm(code, base + start):
            text = f"  0x{ins.address - base:06X}  {ins.mnemonic:8} {ins.op_str}"
            if grep and grep not in text.lower():
                continue
            mark = " <<< 崩溃点" if ins.address == base + rva else ""
            print(text + mark)


if __name__ == "__main__":
    main()
