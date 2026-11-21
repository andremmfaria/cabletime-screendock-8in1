import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/mac/USB Display.app/Contents/MacOS/USB Display"
out_path = "/tmp/vtable_resolved.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()
    fm = program.getFunctionManager()
    mem = program.getMemory()
    st = program.getSymbolTable()

    all_syms = sorted(list(st.getAllSymbols(True)), key=lambda s: s.getAddress().getOffset())
    def nearest_preceding_class_symbol(addr):
        off = addr.getOffset()
        best = None
        for s in all_syms:
            n = s.getName()
            if ("_TtC" in n or "_TtCC" in n) and s.getAddress().getOffset() <= off:
                best = s
            if s.getAddress().getOffset() > off:
                break
        return best.getName() if best else None

    data_iter = listing.getDefinedData(True)
    headers = []
    while data_iter.hasNext():
        d = data_iter.next()
        if "vtable" in d.getDataType().getName().lower():
            headers.append(d)

    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        f.write("Found %d vtable descriptor headers\n" % len(headers))
        for d in headers:
            addr = d.getAddress()
            try:
                vtable_offset = mem.getInt(addr) & 0xFFFFFFFF
                vtable_size = mem.getInt(addr.add(4)) & 0xFFFFFFFF
            except Exception as e:
                f.write("header @ %s read failed: %s\n" % (addr, e))
                continue
            owner = nearest_preceding_class_symbol(addr)
            if vtable_size == 0 or vtable_size > 200:
                continue
            f.write("\n=== vtable @ %s  size=%d  owner-ish=%s ===\n" % (addr, vtable_size, owner))
            slot_addr = addr.add(8)
            for i in range(vtable_size):
                try:
                    rel = mem.getInt(slot_addr)
                except Exception:
                    break
                target_off = slot_addr.getOffset() + rel
                try:
                    target_addr = program.getAddressFactory().getDefaultAddressSpace().getAddress(target_off)
                    fn = fm.getFunctionContaining(target_addr)
                except Exception:
                    fn = None
                f.write("  slot %2d @ %s rel=%d -> 0x%x  func=%s\n" % (
                    i, slot_addr, rel, target_off, fn.getName() if fn else None))
                slot_addr = slot_addr.add(4)

print("WROTE", out_path)
