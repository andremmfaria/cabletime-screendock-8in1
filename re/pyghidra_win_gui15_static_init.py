import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_static_init.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    af = program.getAddressFactory()

    with open(out_path, "w") as f:
        start = af.getAddress(hex(0x00461a00))
        end = af.getAddress(hex(0x00461d00))
        instr = listing.getInstructions(start, True)
        while instr.hasNext():
            ins = instr.next()
            if ins.getAddress().compareTo(end) > 0:
                break
            # print instruction plus any referenced string operand
            refs = ins.getReferencesFrom()
            extra = ""
            for r in refs:
                d = listing.getDataAt(r.getToAddress())
                if d is not None:
                    try:
                        v = d.getValue()
                        if isinstance(v, str):
                            extra += " ; -> %r" % v[:80]
                    except Exception:
                        pass
            f.write("%s: %s%s\n" % (ins.getAddress(), ins.toString(), extra))

print("DONE", out_path)
