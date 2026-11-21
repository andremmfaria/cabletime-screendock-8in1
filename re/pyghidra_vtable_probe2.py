import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/mac/USB Display.app/Contents/MacOS/USB Display"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    fm = program.getFunctionManager()
    mem = program.getMemory()

    # Find every Data item whose data type is TargetVTableDescriptorHeader (or contains 'VTable')
    found = []
    data_iter = listing.getDefinedData(True)
    while data_iter.hasNext():
        d = data_iter.next()
        dtname = d.getDataType().getName()
        if "vtable" in dtname.lower():
            found.append(d)

    print("Found", len(found), "VTable-typed data items")
    for d in found[:40]:
        print(" ", d.getAddress(), d.getDataType().getName(), d.getLabel())

    # For each, dump the surrounding 200 bytes as pointers and show what they resolve to
    for d in found[:10]:
        addr = d.getAddress()
        print("\n--- VTable header at", addr, "---")
        try:
            print(d.toString())
        except Exception as e:
            print("toString failed:", e)
        # scan forward 64 pointer slots after the header for function-looking pointers
        try:
            header_len = d.getLength()
        except Exception:
            header_len = 8
        scan_addr = addr.add(header_len)
        decomp = DecompInterface()
        decomp.openProgram(program)
        for i in range(40):
            try:
                val = mem.getLong(scan_addr)
            except Exception:
                break
            target_addr = program.getAddressFactory().getDefaultAddressSpace().getAddress(val)
            fn = fm.getFunctionContaining(target_addr) if val != 0 else None
            if fn is not None:
                print("  slot %d -> 0x%x -> function %s" % (i, val, fn.getName()))
            scan_addr = scan_addr.add(8)
