import pyghidra, os
pyghidra.start()

from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/mac/USB Display.app/Contents/MacOS/USB Display"
out_path = "/tmp/pyghidra_findings.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    st = program.getSymbolTable()

    targets = ["packetiz", "packet", "surface", "streaming", "fakemouse",
               "hidevent", "cgeventpost", "cgeventcreatemouse", "process", "encode", "compress"]

    funcs = list(fm.getFunctions(True))
    with open(out_path, "w") as f:
        f.write("Total functions: %d\n" % len(funcs))
        matched = [fn for fn in funcs if any(t in fn.getName().lower() for t in targets)]
        f.write("Matched functions (by name): %d\n" % len(matched))
        for fn in matched[:50]:
            f.write("  %s @ %s\n" % (fn.getName(), fn.getEntryPoint()))

        # Also scan ALL symbols (not just functions) for class names, then
        # look at bytes right after each class symbol for function-pointer-like
        # values that fall inside this binary's text range, as a cheap vtable probe.
        all_syms = list(st.getAllSymbols(True))
        class_syms = [s for s in all_syms if any(t in s.getName().lower() for t in
                      ["amhidstreamingpacketizer", "amhidstreamingpacket", "amhidstreamingpipeline", "amhideventfakemouseprocessor"])]
        f.write("\nClass-related symbols found: %d\n" % len(class_syms))
        mem = program.getMemory()
        base = program.getImageBase()
        for s in class_syms[:30]:
            addr = s.getAddress()
            f.write("\n%s @ %s\n" % (s.getName(), addr))

        decomp = DecompInterface()
        decomp.openProgram(program)
        count = 0
        for fn in matched:
            if count >= 10:
                break
            f.write("\n----- %s @ %s -----\n" % (fn.getName(), fn.getEntryPoint()))
            res = decomp.decompileFunction(fn, 25, flat_api.getMonitor())
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC()[:2500])
            else:
                f.write("(failed: %s)\n" % res.getErrorMessage())
            count += 1

print("DONE, wrote", out_path)
