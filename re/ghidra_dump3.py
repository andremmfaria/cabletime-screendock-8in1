script_args = getScriptArgs()
out_path = script_args[0] if script_args else "/tmp/ghidra_findings3.txt"
f = open(out_path, "w")

st = currentProgram.getSymbolTable()
fm = currentProgram.getFunctionManager()
targets = ["fakemouse", "streaming", "hidevent", "cgeventpost", "cgeventcreatemouse", "amhiddevice"]

all_syms = list(st.getAllSymbols(True))
f.write("Total symbols: %d\n" % len(all_syms))
matched = [s for s in all_syms if any(t in s.getName().lower() for t in targets)]
f.write("Matched symbols: %d\n" % len(matched))
seen_funcs = set()
for s in matched[:400]:
    addr = s.getAddress()
    func = fm.getFunctionContaining(addr)
    f.write("\nSYMBOL: %s  @ %s  (func: %s)\n" % (s.getName(), addr, func.getName() if func else None))
    if func is not None and func.getEntryPoint() not in seen_funcs:
        seen_funcs.add(func.getEntryPoint())

f.write("\n\n===== Unique containing functions: %d =====\n" % len(seen_funcs))

from ghidra.app.decompiler import DecompInterface
decomp = DecompInterface()
decomp.openProgram(currentProgram)
count = 0
for entry in seen_funcs:
    if count >= 15:
        f.write("\n... (stopping after 15 decompiles) ...\n")
        break
    func = fm.getFunctionAt(entry)
    f.write("\n----- %s @ %s -----\n" % (func.getName(), entry))
    res = decomp.decompileFunction(func, 25, monitor)
    if res.decompileCompleted():
        f.write(res.getDecompiledFunction().getC()[:3000])
    else:
        f.write("(decompile failed: %s)\n" % res.getErrorMessage())
    count += 1

f.close()
print("WROTE: " + out_path)
