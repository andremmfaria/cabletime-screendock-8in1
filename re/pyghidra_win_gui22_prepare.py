import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_prepare.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        fn = fm.getFunctionContaining(af.getAddress(hex(0x0052a619)))
        f.write("=== %s @ %s ===\n" % (fn.getName(), fn.getEntryPoint()))
        res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
        if res.decompleteCompleted if False else res.decompileCompleted():
            f.write(res.getDecompiledFunction().getC()[:20000])
        else:
            f.write("(decompile failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
