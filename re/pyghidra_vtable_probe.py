import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/mac/USB Display.app/Contents/MacOS/USB Display"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    st = program.getSymbolTable()
    fm = program.getFunctionManager()
    dtm = program.getDataTypeManager()

    all_syms = list(st.getAllSymbols(True))
    print("Total symbols:", len(all_syms))

    vt_syms = [s for s in all_syms if any(t in s.getName().lower() for t in
               ["vtable", "witness", "vwt", "metadata", "fielddescriptor", "nominaltype"])]
    print("vtable/witness/metadata-ish symbols:", len(vt_syms))
    for s in vt_syms[:60]:
        print(" ", s.getName(), "@", s.getAddress(), "type:", s.getSymbolType())

    # Also list any Ghidra-defined data types with 'swift' in the name (structures the analyzer may have created)
    dts = []
    it = dtm.getAllDataTypes()
    while it.hasNext():
        dt = it.next()
        if "swift" in dt.getName().lower() or "vtable" in dt.getName().lower():
            dts.append(dt.getName())
    print("\nSwift/vtable-ish data types defined:", len(dts))
    for n in sorted(set(dts))[:60]:
        print(" ", n)
