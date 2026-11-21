import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_find_ctor.txt"

# candidate no-params-field method-value std::string objects (24-byte MSVC std::string, static)
TARGETS = [0x009f1f30, 0x009f1f60, 0x009f1f78, 0x009f2020, 0x009f1ea0, 0x009f1e58, 0x009f1e40]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    af = program.getAddressFactory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        for t in TARGETS:
            addr = af.getAddress(hex(t))
            refs = refmgr.getReferencesTo(addr)
            f.write("\n=== refs to 0x%08x ===\n" % t)
            funcs = set()
            for r in refs:
                cfn = fm.getFunctionContaining(r.getFromAddress())
                f.write("  from %s type=%s func=%s\n" % (r.getFromAddress(), r.getReferenceType(), cfn.getName() if cfn else None))
                if cfn:
                    funcs.add(cfn)
            for cfn in funcs:
                f.write("\n  --- decompile %s @ %s ---\n" % (cfn.getName(), cfn.getEntryPoint()))
                res = decomp.decompileFunction(cfn, 30, flat_api.getMonitor())
                if res.decompileCompleted():
                    f.write(res.getDecompiledFunction().getC()[:4000])

print("DONE", out_path)
