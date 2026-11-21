import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_setvideobuffer_disasm.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    listing = program.getListing()
    decomp = DecompInterface()
    decomp.openProgram(program)

    # SetVideoBuffer = FUN_0052923f per prior pass notes
    targets = {
        "SetVideoBuffer": "0x0052923f",
        "FUN_00529791": "0x00529791",
        "send_thread_FUN_00529a68": "0x00529a68",
    }

    with open(out_path, "w") as f:
        for name, addr_hex in targets.items():
            fn = fm.getFunctionAt(af.getAddress(addr_hex))
            if fn is None:
                f.write("=== %s: no function at %s ===\n\n" % (name, addr_hex))
                continue
            f.write("=== %s (%s) @ %s ===\n" % (name, fn.getName(), fn.getEntryPoint()))
            res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC()[:12000])
            else:
                f.write("(decompile failed: %s)\n" % res.getErrorMessage())
            f.write("\n\n--- raw disasm ---\n")
            body = fn.getBody()
            for instr in listing.getInstructions(body, True):
                f.write("%s:  %s\n" % (instr.getAddress(), instr.toString()))
            f.write("\n\n")

print("DONE", out_path)
