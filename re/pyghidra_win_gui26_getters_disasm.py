import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_getters_disasm.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    listing = program.getListing()
    st = program.getSymbolTable()
    mem = program.getMemory()

    with open(out_path, "w") as f:
        # Find H264Encoder::vftable via namespace, same approach as prior pass (RTTI namespace lookup)
        syms = [s for s in st.getAllSymbols(True) if s.getName() == "vftable"]
        h264_vftables = [s for s in syms if "H264Encoder" in str(s.getParentNamespace())]
        f.write("H264Encoder vftable syms: %d\n" % len(h264_vftables))
        for s in h264_vftables:
            f.write("  %s @ %s ns=%s\n" % (s.getName(), s.getAddress(), s.getParentNamespace()))

        for s in h264_vftables:
            addr = s.getAddress()
            for off in (0x0c, 0x10):
                slot_addr = addr.add(off)
                ptr = mem.getInt(slot_addr) & 0xffffffff
                taddr = af.getAddress(hex(ptr))
                fn = fm.getFunctionAt(taddr)
                f.write("\n=== slot +0x%x -> %s @ %s ===\n" % (off, fn.getName() if fn else "???", taddr))
                if fn:
                    body = fn.getBody()
                    for instr in listing.getInstructions(body, True):
                        f.write("%s:  %s\n" % (instr.getAddress(), instr.toString()))

print("DONE", out_path)
