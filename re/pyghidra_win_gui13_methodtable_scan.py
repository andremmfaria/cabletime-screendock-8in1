import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_methodtable_scan.txt"

START = 0x009f1d00
END = 0x009f2400

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    af = program.getAddressFactory()
    mem = program.getMemory()

    with open(out_path, "w") as f:
        a = START
        while a < END:
            addr = af.getAddress(hex(a))
            try:
                word = mem.getInt(addr) & 0xffffffff
            except Exception as e:
                f.write("%s | <mem error %s>\n" % (addr, e))
                a += 4
                continue
            s = None
            try:
                taddr = af.getAddress(hex(word))
                data = listing.getDataAt(taddr)
                if data is not None:
                    dt = data.getDataType().getName().lower()
                    if "string" in dt or "unicode" in dt:
                        v = data.getValue()
                        if isinstance(v, str):
                            s = v
            except Exception:
                pass
            f.write("%s | word=0x%08x | str=%r\n" % (addr, word, s))
            a += 4

print("DONE", out_path)
