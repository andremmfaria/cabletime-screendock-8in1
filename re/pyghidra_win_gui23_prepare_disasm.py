import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_prepare_disasm.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    listing = program.getListing()

    fn = fm.getFunctionContaining(af.getAddress(hex(0x0052a619)))
    entry = fn.getEntryPoint()
    body = fn.getBody()

    with open(out_path, "w") as f:
        f.write("=== %s @ %s, body ranges: %s ===\n" % (fn.getName(), entry, list(body.getAddressRanges())))
        instrs = listing.getInstructions(body, True)
        for instr in instrs:
            addr = instr.getAddress()
            mnem = instr.toString()
            f.write("%s:  %s\n" % (addr, mnem))

print("DONE", out_path)
