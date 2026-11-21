import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/mac/USB Display.app/Contents/MacOS/USB Display"
out_path = "/tmp/pyghidra_findings2.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    st = program.getSymbolTable()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        # 1) The packetizer's own process method, by address found earlier
        addr_factory = program.getAddressFactory()
        for a in ["1000f1e70"]:
            addr = addr_factory.getAddress(a)
            fn = fm.getFunctionContaining(addr)
            f.write("\n===== function containing %s: %s =====\n" % (a, fn))
            if fn:
                res = decomp.decompileFunction(fn, 25, flat_api.getMonitor())
                if res.decompileCompleted():
                    f.write(res.getDecompiledFunction().getC()[:4000])
                else:
                    f.write("(failed: %s)\n" % res.getErrorMessage())

        # 2) Callers of VTCompressionSessionCreate, to see codec type constant
        for name in ["_VTCompressionSessionCreate", "_VTCompressionSessionEncodeFrame", "_VTCompressionSessionEncodeFrameWithOutputHandler"]:
            syms = st.getSymbols(name)
            f.write("\n\n===== Callers of %s =====\n" % name)
            for sym in syms:
                refs = refmgr.getReferencesTo(sym.getAddress())
                callers = set()
                for r in refs:
                    cfn = fm.getFunctionContaining(r.getFromAddress())
                    if cfn:
                        callers.add(cfn)
                for cfn in callers:
                    f.write("\n--- caller %s @ %s ---\n" % (cfn.getName(), cfn.getEntryPoint()))
                    res = decomp.decompileFunction(cfn, 25, flat_api.getMonitor())
                    if res.decompileCompleted():
                        f.write(res.getDecompiledFunction().getC()[:4000])
                    else:
                        f.write("(failed: %s)\n" % res.getErrorMessage())

        # 3) compressRawImage: full impl (the real encode entry point)
        funcs = list(fm.getFunctions(True))
        for fn in funcs:
            if fn.getName().startswith("compressRawImage:pts:duration:forceKeyFrame:"):
                f.write("\n\n===== %s @ %s =====\n" % (fn.getName(), fn.getEntryPoint()))
                res = decomp.decompileFunction(fn, 25, flat_api.getMonitor())
                if res.decompileCompleted():
                    f.write(res.getDecompiledFunction().getC()[:5000])
                else:
                    f.write("(failed: %s)\n" % res.getErrorMessage())

print("DONE")
