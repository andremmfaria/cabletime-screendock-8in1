import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_lowlevelwrite_disasm.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    listing = program.getListing()
    decomp = DecompInterface()
    decomp.openProgram(program)

    fn = fm.getFunctionAt(af.getAddress("0x00525cca"))
    with open(out_path, "w") as f:
        f.write("=== %s @ %s ===\n" % (fn.getName(), fn.getEntryPoint()))
        res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
        if res.decompileCompleted():
            f.write(res.getDecompiledFunction().getC()[:20000])
        else:
            f.write("(decompile failed: %s)\n" % res.getErrorMessage())
        f.write("\n\n--- raw disasm ---\n")
        body = fn.getBody()
        for instr in listing.getInstructions(body, True):
            f.write("%s:  %s\n" % (instr.getAddress(), instr.toString()))

print("DONE", out_path)
