import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USBDisplayService.exe"
out_path = "/tmp/svc_main_full.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    decomp = DecompInterface()
    decomp.openProgram(program)

    addr_factory = program.getAddressFactory()
    addr = addr_factory.getAddress("0040407c")
    fn = fm.getFunctionContaining(addr)

    with open(out_path, "w") as f:
        res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
        if res.decompileCompleted():
            f.write(res.getDecompiledFunction().getC())
        else:
            f.write("FAILED: %s" % res.getErrorMessage())

print("DONE", out_path)
