import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_findings2.txt"

TARGETS = ["00525cca", "0052b624", "004beab0", "00529a68", "0052b3f2"]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    decomp = DecompInterface()
    decomp.openProgram(program)
    addr_factory = program.getAddressFactory()

    with open(out_path, "w") as f:
        for a in TARGETS:
            addr = addr_factory.getAddress(a)
            fn = fm.getFunctionContaining(addr)
            f.write("\n\n===== %s -> %s @ %s =====\n" % (a, fn, fn.getEntryPoint() if fn else None))
            if fn:
                res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
                if res.decompileCompleted():
                    f.write(res.getDecompiledFunction().getC())
                else:
                    f.write("(failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
