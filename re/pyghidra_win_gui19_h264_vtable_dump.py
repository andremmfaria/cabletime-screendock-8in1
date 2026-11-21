import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_h264_vtable_dump.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    st = program.getSymbolTable()
    fm = program.getFunctionManager()
    mem = program.getMemory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        syms = [s for s in st.getAllSymbols(True) if "vftable" in s.getName().lower()]
        f.write("TOTAL vftable-named symbols: %d\n" % len(syms))
        for s in syms:
            f.write("SYM %s @ %s\n" % (s.getName(), s.getAddress()))

        f.write("\n\n=== dump slots for each ===\n")
        for s in syms:
            addr = s.getAddress()
            f.write("\n--- %s @ %s ---\n" % (s.getName(), addr))
            for i in range(16):
                slot_addr = addr.add(i*4)
                try:
                    ptr = mem.getInt(slot_addr) & 0xffffffff
                except Exception as e:
                    f.write("  slot[%d] <mem err %s>\n" % (i, e))
                    continue
                try:
                    taddr = program.getAddressFactory().getAddress(hex(ptr))
                    fn = fm.getFunctionAt(taddr)
                    fname = fn.getName() if fn else "???"
                except Exception as e:
                    fname = "ERR:%s" % e
                f.write("  slot[%d] (+0x%x) -> 0x%x  %s\n" % (i, i*4, ptr, fname))

        f.write("\n\n=== decompile H264Encoder vftable slots 0x0c/0x10 (if found) ===\n")
        for s in syms:
            if "H264Encoder" in s.getName():
                addr = s.getAddress()
                for off in (0x0c, 0x10):
                    slot_addr = addr.add(off)
                    ptr = mem.getInt(slot_addr) & 0xffffffff
                    taddr = program.getAddressFactory().getAddress(hex(ptr))
                    fn = fm.getFunctionAt(taddr)
                    if fn:
                        f.write("\n--- slot +0x%x -> %s @ %s ---\n" % (off, fn.getName(), fn.getEntryPoint()))
                        res = decomp.decompileFunction(fn, 40, flat_api.getMonitor())
                        if res.decompileCompleted():
                            f.write(res.getDecompiledFunction().getC()[:6000])
                        else:
                            f.write("(decompile failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
