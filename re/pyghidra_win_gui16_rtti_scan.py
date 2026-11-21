import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_rtti_scan.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        data_iter = listing.getDefinedData(True)
        hits = []
        while data_iter.hasNext():
            data = data_iter.next()
            dt_name = data.getDataType().getName().lower()
            if "string" not in dt_name and "unicode" not in dt_name:
                continue
            try:
                val = data.getValue()
            except Exception:
                continue
            if not isinstance(val, str):
                continue
            low = val.lower()
            if "encoder" in low or "nvenc" in low or "nvidia" in low or "amf" in low or "qsv" in low:
                hits.append((data.getAddress(), val))

        f.write("TOTAL: %d\n" % len(hits))
        for addr, val in hits:
            f.write("%s | %r\n" % (addr, val[:200]))

        f.write("\n\n=== decompile functions referencing each hit ===\n")
        for addr, val in hits:
            refs = refmgr.getReferencesTo(addr)
            callers = set()
            for r in refs:
                cfn = fm.getFunctionContaining(r.getFromAddress())
                if cfn:
                    callers.add(cfn)
            if callers:
                f.write("\n--- string %r @ %s ---\n" % (val[:100], addr))
                for cfn in callers:
                    f.write("  referenced by %s @ %s\n" % (cfn.getName(), cfn.getEntryPoint()))

print("DONE", out_path)
