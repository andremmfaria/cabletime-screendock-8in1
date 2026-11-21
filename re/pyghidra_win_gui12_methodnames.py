import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_methodnames.txt"

# candidate "method" value global addresses seen as args to FUN_004c2f00 in the
# various JSON-RPC-message-builder functions found referencing the 'jsonrpc' string
CANDIDATES = [
    0x009f1e10,  # array, indexed by iVar2*0x18 in FUN_0051b4a2/FUN_0051b65e
    0x009f1e28,
    0x009f1e40,  # FUN_0051bbdf
    0x009f1e58,  # FUN_0051b8d7
    0x009f1e70,
    0x009f1e88,
    0x009f1ea0,  # FUN_0051b7a4
    0x009f1eb8,
    0x009f1ed0,
    0x009f1ee8,  # FUN_0051c13d ("version")
    0x009f1f00,
    0x009f1f18,
    0x009f1f30,  # FUN_0051c39c
    0x009f1f48,
    0x009f1f60,  # FUN_0051c4cf
    0x009f1f78,  # FUN_0051c602
    0x009f1f90,
    0x009f1fa8,
    0x009f1fc0,
    0x009f1fd8,
    0x009f1ff0,
    0x009f2008,
    0x009f2020,  # FUN_0051d030
    0x009f2038,
    0x009f2050,
    0x009f2068,
    0x009f2080,
    0x009f2098,
    0x009f20b0,
    0x009f20c8,
    0x009f20d0,  # array in FUN_0051c13d (method index by param_2*0x18)
]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    af = program.getAddressFactory()
    mem = program.getMemory()

    with open(out_path, "w") as f:
        for a in CANDIDATES:
            addr = af.getAddress(hex(a))
            data = listing.getDataAt(addr)
            info = "no-data"
            val = None
            if data is not None:
                info = data.getDataType().getName()
                try:
                    val = data.getValue()
                except Exception as e:
                    val = "ERR:%s" % e
            f.write("%s | datatype=%s | value=%r\n" % (addr, info, val))

print("DONE", out_path)
