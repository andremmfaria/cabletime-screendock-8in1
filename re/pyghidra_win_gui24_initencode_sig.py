import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_initencode_sig.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    af = program.getAddressFactory()
    listing = program.getListing()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        # H264Encoder::InitEncodeH264 = FUN_00549ab9 per prior pass notes
        for addr_hex in ("0x00549ab9",):
            fn = fm.getFunctionAt(af.getAddress(addr_hex))
            if fn is None:
                f.write("no function at %s\n" % addr_hex)
                continue
            f.write("=== %s @ %s  params=%s calling_conv=%s ===\n" % (
                fn.getName(), fn.getEntryPoint(),
                [ (p.getName(), str(p.getDataType())) for p in fn.getParameters() ],
                fn.getCallingConventionName()))
            res = decomp.decompileFunction(fn, 40, flat_api.getMonitor())
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC()[:6000])
            else:
                f.write("(decompile failed: %s)\n" % res.getErrorMessage())
            f.write("\n\n--- raw disasm of first 80 instrs ---\n")
            body = fn.getBody()
            instrs = listing.getInstructions(body, True)
            count = 0
            for instr in instrs:
                f.write("%s:  %s\n" % (instr.getAddress(), instr.toString()))
                count += 1
                if count > 80:
                    break

        # Also dump the vtable slot[0] target function for H264Encoder to confirm identity
        st = program.getSymbolTable()
        mem = program.getMemory()
        syms = [s for s in st.getAllSymbols(True) if s.getName() == "vftable" and "H264Encoder" in str(s.getParentNamespace())]
        for s in syms:
            addr = s.getAddress()
            ptr = mem.getInt(addr) & 0xffffffff
            taddr = af.getAddress(hex(ptr))
            fn0 = fm.getFunctionAt(taddr)
            f.write("\nH264Encoder::vftable @ %s slot[0] -> 0x%x (%s)\n" % (addr, ptr, fn0.getName() if fn0 else "???"))

print("DONE", out_path)
