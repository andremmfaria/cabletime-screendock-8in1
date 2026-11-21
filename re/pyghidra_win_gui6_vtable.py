import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_vtable_scan.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    st = program.getSymbolTable()
    fm = program.getFunctionManager()
    listing = program.getListing()
    mem = program.getMemory()

    with open(out_path, "w") as f:
        # Find vftable symbols for the encoder classes
        for cls in ["H264Encoder", "AMFEncoder", "QSVEncoder", "IEncoder", "Encoder"]:
            syms = [s for s in st.getAllSymbols(True) if "vftable" in s.getName() and cls in s.getName()]
            for s in syms:
                f.write("VFTABLE %s @ %s\n" % (s.getName(), s.getAddress()))
                addr = s.getAddress()
                # dump first 0x30 bytes as function pointers (12 slots)
                try:
                    for i in range(14):
                        slot_addr = addr.add(i*4)
                        ptr = mem.getInt(slot_addr)
                        # interpret as address
                        try:
                            target_addr = program.getAddressFactory().getAddress(hex(ptr & 0xffffffff))
                            fn = fm.getFunctionAt(target_addr)
                            fname = fn.getName() if fn else "???"
                        except Exception as e:
                            fname = "ERR:%s" % e
                        f.write("  slot[%d] (+0x%x) -> 0x%x  %s\n" % (i, i*4, ptr & 0xffffffff, fname))
                except Exception as e:
                    f.write("  (error reading vtable: %s)\n" % e)
        f.write("\n--- end ---\n")

print("DONE", out_path)
