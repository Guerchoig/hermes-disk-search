"""Список экспортов PE-файла (чистый stdlib) — разведка C-API движка.

Полезно для W3 (аудио-API) и вообще для поиска имён символов в DLL движка.

Запуск из корня репозитория:
  .\\.venv\\Scripts\\python.exe tools\\parity\\pe_exports.py <path.dll> [подстрока]
Печатает имена экспортов (при подстроке — только совпадающие); число — в stderr.
"""
import struct
import sys


def exports(path):
    d = open(path, "rb").read()
    if d[:2] != b"MZ":
        return []
    e = struct.unpack_from("<I", d, 0x3C)[0]
    if d[e:e + 4] != b"PE\x00\x00":
        return []
    coff = e + 4
    _machine, nsec, _ts, _sym, _nsym, optsize, _ch = struct.unpack_from("<HHIIIHH", d, coff)
    opt = coff + 20
    magic = struct.unpack_from("<H", d, opt)[0]
    if magic == 0x20B:            # PE32+
        dd = opt + 112
    elif magic == 0x10B:          # PE32
        dd = opt + 96
    else:
        return []
    exp_rva = struct.unpack_from("<I", d, dd)[0]  # data directory[0] = export
    sec = opt + optsize
    sections = []
    for i in range(nsec):
        o = sec + i * 40
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", d, o + 8)
        sections.append((vaddr, vsize, rawptr, rawsize))

    def rva2off(rva):
        for vaddr, vsize, rawptr, rawsize in sections:
            if vaddr <= rva < vaddr + max(vsize, rawsize):
                return rawptr + (rva - vaddr)
        return None

    off = rva2off(exp_rva)
    if off is None:
        return []
    num_names = struct.unpack_from("<I", d, off + 24)[0]
    addr_names = struct.unpack_from("<I", d, off + 32)[0]
    on = rva2off(addr_names)
    names = []
    for i in range(num_names):
        nrva = struct.unpack_from("<I", d, on + i * 4)[0]
        no = rva2off(nrva)
        end = d.index(b"\x00", no)
        names.append(d[no:end].decode("ascii", "replace"))
    return names


if __name__ == "__main__":
    path = sys.argv[1]
    pat = (sys.argv[2] if len(sys.argv) > 2 else "").lower()
    all_names = exports(path)
    shown = [n for n in all_names if pat in n.lower()]
    for n in sorted(shown):
        print(n)
    sys.stderr.write("# exports total=%d shown=%d\n" % (len(all_names), len(shown)))
