script_args = getScriptArgs()
out_path = script_args[0] if script_args else "/tmp/ghidra_findings2.txt"
f = open(out_path, "w")

fm = currentProgram.getFunctionManager()
from ghidra.app.decompiler import DecompInterface
decomp = DecompInterface()
decomp.openProgram(currentProgram)

targets_substr = ["FakeMouse", "AMHIDStreaming", "AMHIDEventProvider", "AMHIDDevice"]

funcs = list(fm.getFunctions(True))
f.write("Total functions: %d\n" % len(funcs))
matched = [fn for fn in funcs if any(s in fn.getName() for s in targets_substr)]
f.write("Matched functions: %d\n" % len(matched))
for fn in matched:
    f.write("\n===== %s @ %s =====\n" % (fn.getName(), fn.getEntryPoint()))
    res = decomp.decompileFunction(fn, 30, monitor)
    if res.decompileCompleted():
        code = res.getDecompiledFunction().getC()
        f.write(code[:6000])
    else:
        f.write("(decompile failed: %s)\n" % res.getErrorMessage())

f.close()
print("WROTE: " + out_path)
