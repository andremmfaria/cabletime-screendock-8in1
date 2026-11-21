import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_requireencoder2.txt"

TARGETS = [0x0054247d, 0x004e3ae5, 0x0054603c, 0x0054620c, 0x00548ed8, 0x00549960, 0x00549ab9]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        for t in TARGETS:
            addr = af.getAddress(hex(t))
            fn = fm.getFunctionContaining(addr)
            if fn is None:
                f.write("\n=== 0x%08x: no function ===\n" % t)
                continue
            f.write("\n=== %s @ %s ===\n" % (fn.getName(), fn.getEntryPoint()))
            res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC()[:16000])
            else:
                f.write("(decompile failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
