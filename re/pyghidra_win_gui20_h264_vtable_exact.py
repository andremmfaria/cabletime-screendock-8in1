import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_h264_vtable_exact.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    st = program.getSymbolTable()
    fm = program.getFunctionManager()
    mem = program.getMemory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        syms = [s for s in st.getAllSymbols(True) if "H264Encoder" in s.getName() and "vftable" in s.getName()]
        f.write("MATCHES: %d\n" % len(syms))
        for s in syms:
            f.write("SYM %s @ %s (ns=%s)\n" % (s.getName(), s.getAddress(), s.getParentNamespace()))

        for s in syms:
            addr = s.getAddress()
            f.write("\n--- slots for %s @ %s ---\n" % (s.getName(), addr))
            for i in range(16):
                slot_addr = addr.add(i*4)
                ptr = mem.getInt(slot_addr) & 0xffffffff
                taddr = program.getAddressFactory().getAddress(hex(ptr))
                fn = fm.getFunctionAt(taddr)
                fname = fn.getName() if fn else "???"
                f.write("  slot[%d] (+0x%x) -> 0x%x  %s\n" % (i, i*4, ptr, fname))

            # decompile slots +0x0c and +0x10
            for off in (0x0c, 0x10):
                slot_addr = addr.add(off)
                ptr = mem.getInt(slot_addr) & 0xffffffff
                taddr = program.getAddressFactory().getAddress(hex(ptr))
                fn = fm.getFunctionAt(taddr)
                if fn:
                    f.write("\n  >>> decompile slot +0x%x -> %s @ %s <<<\n" % (off, fn.getName(), fn.getEntryPoint()))
                    res = decomp.decompileFunction(fn, 40, flat_api.getMonitor())
                    if res.decompileCompleted():
                        f.write(res.getDecompiledFunction().getC()[:4000])
                    else:
                        f.write("(decompile failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
